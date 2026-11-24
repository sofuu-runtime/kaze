/* kaze_board_sim.c -- see kaze_board_sim.h.
 *
 * One register accessor with two backends: a small behavioural model of the peripherals
 * the Phase 14 drivers talk to (host suites and the QEMU image both link this), and, on
 * metal, a real SysTick for time. Everything else is modelled, because the emulated board
 * (lm3s6965evb) has a UART, SysTick and GPIO and nothing else - which is exactly why the
 * drivers are written against a register interface instead of a vendor SDK.
 *
 * The register file is open-addressed and fixed-size: no allocation, so this compiles for
 * a Cortex-M as it stands.
 */
#include "kaze_board_sim.h"

#ifdef KAZE_SIM_METAL
#define SIM_HOST 0
#else
#include <stdio.h>
#include <string.h>
#define SIM_HOST 1
#endif

/* ---- register file ---------------------------------------------------- */
#define RF_SLOTS 1024u
static uint32_t rf_addr[RF_SLOTS];
static uint32_t rf_val[RF_SLOTS];
static unsigned char rf_used[RF_SLOTS];

static uint32_t rf_home(uint32_t a) {
    uint32_t h = (a >> 2) * 2654435761u;
    return (h >> 13) & (RF_SLOTS - 1u);
}
static int rf_find(uint32_t a) {
    uint32_t i = rf_home(a);
    while (rf_used[i]) {
        if (rf_addr[i] == a) return (int)i;
        i = (i + 1u) & (RF_SLOTS - 1u);
    }
    return -1;
}
static uint32_t rf_get(uint32_t a) {
    int i = rf_find(a);
    return (i < 0) ? 0u : rf_val[i];
}
static void rf_set(uint32_t a, uint32_t v) {
    int i = rf_find(a);
    if (i >= 0) { rf_val[i] = v; return; }
    uint32_t j = rf_home(a);
    while (rf_used[j]) { j = (j + 1u) & (RF_SLOTS - 1u); }
    rf_used[j] = 1u; rf_addr[j] = a; rf_val[j] = v;
}
static void rf_clear(void) {
    for (uint32_t i = 0; i < RF_SLOTS; i++) { rf_used[i] = 0; rf_addr[i] = 0; rf_val[i] = 0; }
}

/* ---- counters --------------------------------------------------------- */
static uint32_t sim_cyc;            /* host clock: advanced per access */
static uint32_t unknown_accesses;
static uint32_t bad_align;
static uint32_t extra_access_cost = 8u;

uint32_t kaze_sim_cycles(void) {
#if SIM_HOST
    return sim_cyc;
#else
    /* SysTick is the emulator's: with -icount each instruction is one count, so this is an
     * instruction-accurate clock. It is configured lazily with the longest reload so a
     * driver cannot miss a wrap between two reads. */
    volatile uint32_t* csr = (volatile uint32_t*)SIM_SYST_CSR;
    volatile uint32_t* rvr = (volatile uint32_t*)SIM_SYST_RVR;
    volatile uint32_t* cvr = (volatile uint32_t*)SIM_SYST_CVR;
    static uint32_t prev;
    static uint32_t hi;
    /* Configure on the first call *and* whenever the reload has been left at zero, which is
     * how the core comes out of reset here: CSR already reads 4 (clock source set, counter
     * off), so testing the enable bit alone left the period at one and the clock frozen. */
    if (*rvr == 0u) {
        *rvr = 0x00FFFFFFu;
        *cvr = 0u;
        *csr = *csr | 5u;
        prev = 0u; hi = 0u;
        return 0u;
    }
    uint32_t period = (*rvr & 0x00FFFFFFu) + 1u;
    uint32_t now = period - (*cvr & 0x00FFFFFFu);
    if (now < prev) hi += period;
    prev = now;
    return hi + now;
#endif
}

/* ---- actuator edge log ------------------------------------------------ */
#define EDGE_LOG 1024u
static uint32_t edge_time[EDGE_LOG];
static uint32_t edge_value[EDGE_LOG];
static uint32_t edge_count;

static void log_edge(uint32_t value) {
    uint32_t k = edge_count & (EDGE_LOG - 1u);
    edge_time[k] = kaze_sim_cycles();
    edge_value[k] = value;
    edge_count++;
}

/* ---- timer model ------------------------------------------------------ */
static uint32_t tim_cnt_base;

/* ---- SPI + IMU -------------------------------------------------------- */
#define SPI_FIFO 64u
static unsigned char spi_rx[SPI_FIFO];
static uint32_t spi_head, spi_tail;
static unsigned char spi_stream[8];
static uint32_t spi_stream_len, spi_stream_pos;
static uint32_t imu_samples;
static uint32_t imu_phase;

static void spi_push(unsigned char b) {
    if (spi_head - spi_tail < SPI_FIFO) spi_rx[spi_head & (SPI_FIFO - 1u)] = b, spi_head++;
}
static int spi_pop(unsigned char* out) {
    if (spi_head == spi_tail) return 0;
    *out = spi_rx[spi_tail & (SPI_FIFO - 1u)];
    spi_tail++;
    return 1;
}
static void spi_stream_start(unsigned char reg) {
    spi_stream_len = 0u;
    spi_stream_pos = 0u;
    if (reg == 0x75u) {                       /* WHO_AM_I */
        spi_stream[spi_stream_len++] = 0x47u;
        return;
    }
    if (reg >= 0x1Au && reg <= 0x1Fu) {       /* six data bytes, changing every conversion */
        imu_phase = imu_phase * 1103515245u + 12345u;
        for (uint32_t i = 0; i < 6u; i++)
            spi_stream[spi_stream_len++] = (unsigned char)((imu_phase >> (8u * (i & 3u))) & 0xFFu);
        imu_samples++;
        return;
    }
    spi_stream[spi_stream_len++] = 0u;
}

/* ---- I2C + barometer -------------------------------------------------- */
#define BARO_FIFO 64u
static unsigned char i2c_rx[BARO_FIFO];
static uint32_t i2c_head, i2c_tail;
static unsigned char i2c_ptr;
static int i2c_started;
static int i2c_addr_done;
static uint32_t baro_phase;

static void i2c_push(unsigned char b) {
    if (i2c_head - i2c_tail < BARO_FIFO) i2c_rx[i2c_head & (BARO_FIFO - 1u)] = b, i2c_head++;
}
static int i2c_pop(unsigned char* out) {
    if (i2c_head == i2c_tail) return 0;
    *out = i2c_rx[i2c_tail & (BARO_FIFO - 1u)];
    i2c_tail++;
    return 1;
}
static void i2c_read_register(unsigned char reg) {
    baro_phase = baro_phase * 1664525u + 1013904223u;
    switch (reg) {
        case 0x00u: i2c_push(0x50u); break;                       /* chip id: BMP390 */
        case 0x04u:                                                /* 24-bit pressure */
            i2c_push((unsigned char)(baro_phase & 0xFFu));
            i2c_push((unsigned char)((baro_phase >> 8) & 0xFFu));
            i2c_push((unsigned char)((baro_phase >> 16) & 0xFFu));
            break;
        case 0x07u:                                                /* temperature */
            i2c_push((unsigned char)((baro_phase >> 4) & 0xFFu));
            i2c_push((unsigned char)((baro_phase >> 12) & 0xFFu));
            break;
        default: i2c_push(0u); break;
    }
}

/* ---- CAN loopback ----------------------------------------------------- */
static uint32_t can_rx_count;

/* ---- DMA -------------------------------------------------------------- */
typedef struct { uint32_t cr, ndtr, par, m0ar; } dma_ch;
static dma_ch dma[2];
static uint32_t dma_done;

/* ---- UART ------------------------------------------------------------- */
#define UART_LOG 4096u
static char uart_text[UART_LOG];
static uint32_t uart_len;

static void uart_append(unsigned char b) {
    if (uart_len + 1u < UART_LOG) uart_text[uart_len++] = (char)b;
    uart_text[uart_len] = 0;
}

/* ---- RAM window ------------------------------------------------------- */
static unsigned char sim_ram[SIM_RAM_SIZE];

/* ---- SIO (RP2040) ----------------------------------------------------- */
static uint32_t sio_out, sio_oe;

static int in_window(uint32_t a, uint32_t base) {
    return a >= base && a < base + 0x400u;
}

static uint32_t ram_read(uint32_t off, int width) {
    uint32_t v = 0;
    for (int i = 0; i < width; i++) v |= (uint32_t)sim_ram[off + (uint32_t)i] << (8 * i);
    return v;
}
static void ram_write(uint32_t off, int width, uint32_t v) {
    for (int i = 0; i < width; i++) sim_ram[off + (uint32_t)i] = (unsigned char)((v >> (8 * i)) & 0xFFu);
}

static void dma_run(unsigned idx) {
    dma_ch* c = &dma[idx];
    int width = 1;
    uint32_t w = (c->cr >> 10) & 3u;
    if (w == 1u) width = 2;
    if (w == 2u) width = 4;
    int to_periph = (int)((c->cr >> 4) & 1u);
    for (uint32_t i = 0; i < c->ndtr; i++) {
        /* A peripheral address does not advance: the DMA pointer moves through memory and
         * writes the same data register every time. Getting this wrong makes a telemetry
         * stream land one byte past the UART and vanish, which is how it was caught. */
        uint32_t src = to_periph ? (c->m0ar + i * (uint32_t)width) : c->par;
        uint32_t dst = to_periph ? c->par : (c->m0ar + i * (uint32_t)width);
        uint32_t v;
        if (src >= SIM_RAM_BASE && src < SIM_RAM_BASE + SIM_RAM_SIZE) v = ram_read(src - SIM_RAM_BASE, width);
        else v = kaze_board_mmio(src, width, 0, 0);
        if (dst >= SIM_RAM_BASE && dst < SIM_RAM_BASE + SIM_RAM_SIZE) ram_write(dst - SIM_RAM_BASE, width, v);
        else kaze_board_mmio(dst, width, 1, v);
    }
    c->cr &= ~1u;
    dma_done |= (1u << idx);
}

/* ---- the accessor ----------------------------------------------------- */
uint32_t kaze_board_mmio(uint32_t addr, int width, int is_write, uint32_t value) {
#if SIM_HOST
    sim_cyc += extra_access_cost;
#endif
    if (width != 1 && width != 2 && width != 4) { bad_align++; return 0u; }
    if ((width == 4 && (addr & 3u)) || (width == 2 && (addr & 1u))) { bad_align++; return 0u; }

#if !SIM_HOST
    /* SysTick is the emulator's own peripheral, so accesses in its window must reach it
     * rather than the register file: that is the clock the timing gates measure with. */
    if (addr >= SIM_SYST_CSR && addr < SIM_SYST_CSR + 0x10u) {
        volatile uint32_t* reg = (volatile uint32_t*)addr;
        if (is_write) { *reg = value; return value; }
        return *reg;
    }
#endif

    /* RAM the DMA model moves bytes through. */
    if (addr >= SIM_RAM_BASE && addr < SIM_RAM_BASE + SIM_RAM_SIZE) {
        uint32_t off = addr - SIM_RAM_BASE;
        if (is_write) { ram_write(off, width, value); return value; }
        return ram_read(off, width);
    }

    /* The test-only control block. */
    if (in_window(addr, SIM_CTL)) {
        uint32_t off = addr - SIM_CTL;
        if (is_write) {
            if (off == SIM_CTL_EDGE_INDEX || off == SIM_CTL_UART_INDEX) { rf_set(addr, value); return value; }
            if (off == SIM_CTL_RESET && value != 0u) {
                rf_clear();
                edge_count = 0; unknown_accesses = 0; bad_align = 0; uart_len = 0;
                imu_samples = 0; can_rx_count = 0; dma_done = 0; sio_out = 0; sio_oe = 0;
                sim_cyc = 0; tim_cnt_base = 0; i2c_started = 0; i2c_ptr = 0;
                i2c_head = i2c_tail = 0; spi_head = spi_tail = 0;
                uart_text[0] = 0;
            }
            if (off == SIM_CTL_SYSCLK_HZ) rf_set(addr, value);
            return value;
        }
        switch (off) {
            case SIM_CTL_CYCLES: return kaze_sim_cycles();
            case SIM_CTL_EDGE_COUNT: return edge_count;
            case SIM_CTL_EDGE_TIME: {
                uint32_t k = rf_get(SIM_CTL + SIM_CTL_EDGE_INDEX) & (EDGE_LOG - 1u);
                return edge_time[k];
            }
            case SIM_CTL_EDGE_VALUE: {
                uint32_t k = rf_get(SIM_CTL + SIM_CTL_EDGE_INDEX) & (EDGE_LOG - 1u);
                return edge_value[k];
            }
            case SIM_CTL_UNKNOWN: return unknown_accesses;
            case SIM_CTL_BAD_ALIGN: return bad_align;
            case SIM_CTL_UART_LEN: return uart_len;
            case SIM_CTL_UART_BYTE: {
                uint32_t k = rf_get(SIM_CTL + SIM_CTL_UART_INDEX);
                return (k < uart_len) ? (uint32_t)(unsigned char)uart_text[k] : 0u;
            }
            case SIM_CTL_IMU_SAMPLES: return imu_samples;
            case SIM_CTL_CAN_RX: return can_rx_count;
            case SIM_CTL_SYSTICK_REAL:
#if SIM_HOST
                return 0u;
#else
                return 1u;
#endif
            default: break;
        }
        return rf_get(addr);
    }

    /* ---- STM32H7-style timer 2 ---- */
    if (in_window(addr, SIM_TIM2_BASE)) {
        uint32_t off = addr - SIM_TIM2_BASE;
        if (off == 0x24u) {                                   /* CNT */
            if (is_write) { tim_cnt_base = kaze_sim_cycles(); return value; }
            return kaze_sim_cycles() - tim_cnt_base;
        }
        if (off == 0x10u && !is_write) {                      /* SR */
            uint32_t arr = rf_get(SIM_TIM2_BASE + 0x2Cu);
            return ((kaze_sim_cycles() - tim_cnt_base) >= arr) ? 1u : 0u;
        }
        if (is_write && off >= 0x34u && off <= 0x40u) {        /* CCR1..CCR4: an actuator edge */
            log_edge(value);
            rf_set(addr, value);
            return value;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- SPI1 with an IMU on it ---- */
    if (in_window(addr, SIM_SPI1_BASE)) {
        uint32_t off = addr - SIM_SPI1_BASE;
        if (off == 0x08u) {                                   /* SR */
            return (spi_head != spi_tail ? 1u : 0u) | 2u;
        }
        if (off == 0x0Cu) {                                   /* DR */
            if (is_write) {
                unsigned char cmd = (unsigned char)(value & 0xFFu);
                if (cmd & 0x80u) {
                    spi_stream_start((unsigned char)(cmd & 0x7Fu));
                    unsigned char first = 0;
                    if (spi_stream_len > 0u) { first = spi_stream[0]; spi_stream_pos = 1u; }
                    spi_push(first);
                }
                return value;
            }
            unsigned char out = 0;
            spi_pop(&out);
            if (spi_stream_pos < spi_stream_len) spi_push(spi_stream[spi_stream_pos++]);
            return (uint32_t)out;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- I2C1 with a barometer on it ---- */
    if (in_window(addr, SIM_I2C1_BASE)) {
        uint32_t off = addr - SIM_I2C1_BASE;
        if (off == 0x00u) {                                   /* CR1 */
            if (is_write) {
                if (value & (1u << 13)) { i2c_started = 1; i2c_addr_done = 0; }
                if (value & (1u << 14)) { i2c_started = 0; i2c_ptr = 0; i2c_addr_done = 0; }
                rf_set(addr, value);
            }
            return rf_get(addr);
        }
        if (off == 0x18u) {                                   /* ISR */
            return 2u | 0x40u | (i2c_head != i2c_tail ? 4u : 0u);
        }
        if (off == 0x28u && is_write) {                        /* TXDR */
            unsigned char b = (unsigned char)(value & 0xFFu);
            /* The first byte after a START is the address and is consumed; every later
             * write is the register pointer. Without the latch an even register number
             * (0x04, the pressure block) looked exactly like an address and was dropped. */
            if (i2c_started && !i2c_addr_done && (b & 1u) == 0u) {
                i2c_addr_done = 1;
                i2c_head = i2c_tail = 0;      /* a new transaction cannot pop stale bytes */
            } else {
                i2c_ptr = b;
                /* The device answers the register-pointer write by clocking bytes out, so
                 * they are queued here and RXNE is already set when a polled driver looks.
                 * Materialising them on the read instead made a correct driver time out. */
                if (i2c_started) i2c_read_register(i2c_ptr++);
            }
            return value;
        }
        if (off == 0x24u && !is_write) {                       /* RXDR */
            if (i2c_head == i2c_tail) i2c_read_register(i2c_ptr++);
            unsigned char out = 0;
            i2c_pop(&out);
            return (uint32_t)out;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- CAN1 in loopback ---- */
    if (in_window(addr, SIM_CAN1_BASE)) {
        uint32_t off = addr - SIM_CAN1_BASE;
        if (off == 0x08u && is_write && (value & 1u)) {        /* TSR: request completion */
            rf_set(SIM_CAN1_BASE + 0x1B0u, rf_get(SIM_CAN1_BASE + 0x180u));  /* RIR <- TIR */
            rf_set(SIM_CAN1_BASE + 0x1B4u, rf_get(SIM_CAN1_BASE + 0x184u));
            rf_set(SIM_CAN1_BASE + 0x1B8u, rf_get(SIM_CAN1_BASE + 0x188u));
            rf_set(SIM_CAN1_BASE + 0x1BCu, rf_get(SIM_CAN1_BASE + 0x18Cu));
            rf_set(SIM_CAN1_BASE + 0x0Cu, 1u);                 /* RF0R: one frame pending */
            rf_set(addr, 3u);                                  /* TSR: RQCP0 | TXOK0 */
            can_rx_count++;
            return 3u;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- USART1 (the UART the DMA feeds) ---- */
    if (in_window(addr, SIM_USART1_BASE)) {
        uint32_t off = addr - SIM_USART1_BASE;
        /* TDR: a byte write into the data-register window is one byte out, whatever the
         * width the accessor was given. */
        if (is_write && off >= 0x24u && off < 0x2Cu) { uart_append((unsigned char)(value & 0xFFu)); return value; }
        if (off == 0x1Cu) return 0x80u | 0x40u;                /* ISR: TXE | TC */
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- DMA1 ---- */
    if (in_window(addr, SIM_DMA1_BASE)) {
        uint32_t off = addr - SIM_DMA1_BASE;
        if (off == 0x00u && !is_write) return dma_done;        /* LISR */
        if (off >= 0x10u && off < 0x38u) {
            uint32_t idx = (off - 0x10u) / 0x18u;
            uint32_t f = (off - 0x10u) % 0x18u;
            if (idx < 2u) {
                dma_ch* c = &dma[idx];
                if (f == 0x00u) {
                    if (is_write) {
                        c->cr = value;
                        if (value & 1u) dma_run(idx);
                    }
                    return c->cr;
                }
                if (f == 0x04u) { if (is_write) c->ndtr = value; return c->ndtr; }
                if (f == 0x08u) { if (is_write) c->par = value; return c->par; }
                if (f == 0x0Cu) { if (is_write) c->m0ar = value; return c->m0ar; }
            }
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- GPIO ---- */
    if (in_window(addr, SIM_GPIOA_BASE)) {
        uint32_t off = addr - SIM_GPIOA_BASE;
        if (off == 0x18u && is_write) {                        /* BSRR */
            uint32_t odr = rf_get(SIM_GPIOA_BASE + 0x14u);
            odr |= (value & 0xFFFFu);
            odr &= ~((value >> 16) & 0xFFFFu);
            rf_set(SIM_GPIOA_BASE + 0x14u, odr);
            return value;
        }
        if (off == 0x10u) return rf_get(SIM_GPIOA_BASE + 0x14u);  /* IDR mirrors ODR */
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: resets, clocks, pads, io bank ---- */
    if (addr >= SIM_RP_RESETS && addr < SIM_RP_RESETS + 0x400u) {
        uint32_t off = addr - SIM_RP_RESETS;
        if (off == 0x08u) return 0xFFFFFFFFu;                  /* RESET_DONE: all up */
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }
    if (in_window(addr, SIM_RP_CLOCKS) || in_window(addr, SIM_RP_IO_BANK0) || in_window(addr, SIM_RP_PADS)) {
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: PWM (its channels share the actuator edge log) ---- */
    if (in_window(addr, SIM_RP_PWM)) {
        uint32_t off = addr - SIM_RP_PWM;
        if (off < 0xF0u && (off % 0x14u) == 12u && is_write) {   /* CHn_CC */
            log_edge(value);
            rf_set(addr, value);
            return value;
        }
        if (off < 0xF0u && (off % 0x14u) == 8u && !is_write) {   /* CHn_CTR */
            uint32_t base_off = off - 8u;
            uint32_t top = rf_get(SIM_RP_PWM + base_off + 16u);
            if (top == 0u) top = 0xFFFFu;
            return (kaze_sim_cycles() % (top + 1u));
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: hardware timer ---- */
    if (in_window(addr, SIM_RP_TIMER)) {
        uint32_t off = addr - SIM_RP_TIMER;
        uint32_t c = kaze_sim_cycles();
        if (off == 0x08u) return c >> 16;                       /* TIMEHR */
        if (off == 0x0Cu || off == 0x28u) return c;             /* TIMELR / TIMERAWL */
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: SPI0 (second SPI bus, same IMU model) ---- */
    if (in_window(addr, SIM_RP_SPI0)) {
        uint32_t off = addr - SIM_RP_SPI0;
        if (off == 0x0Cu) return 0x2u | (spi_head != spi_tail ? 4u : 0u);  /* SSPSR */
        if (off == 0x08u) {
            if (is_write) {
                unsigned char cmd = (unsigned char)(value & 0xFFu);
                if (cmd & 0x80u) {
                    spi_stream_start((unsigned char)(cmd & 0x7Fu));
                    unsigned char first = 0;
                    if (spi_stream_len > 0u) { first = spi_stream[0]; spi_stream_pos = 1u; }
                    spi_push(first);
                }
                return value;
            }
            unsigned char out = 0;
            spi_pop(&out);
            if (spi_stream_pos < spi_stream_len) spi_push(spi_stream[spi_stream_pos++]);
            return (uint32_t)out;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: I2C0 (same barometer model) ---- */
    if (in_window(addr, SIM_RP_I2C0)) {
        uint32_t off = addr - SIM_RP_I2C0;
        if (off == 0x70u) return 0x2u | 0x4u | (i2c_head != i2c_tail ? 8u : 0u);  /* IC_STATUS */
        if (off == 0x34u) return (i2c_head != i2c_tail ? 8u : 0u);
        if (off == 0x10u) {                                      /* IC_DATA_CMD */
            if (is_write) {
                if (value & 0x100u) i2c_read_register(i2c_ptr++);
                else i2c_ptr = (unsigned char)(value & 0xFFu);
                return value;
            }
            if (i2c_head == i2c_tail) i2c_read_register(i2c_ptr++);
            unsigned char out = 0;
            i2c_pop(&out);
            return (uint32_t)out;
        }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: SIO ---- */
    if (in_window(addr, SIM_RP_SIO)) {
        uint32_t off = addr - SIM_RP_SIO;
        if (off == 0x010u) { if (is_write) sio_out = value; return sio_out; }
        if (off == 0x014u && is_write) { sio_out |= value; return sio_out; }
        if (off == 0x018u && is_write) { sio_out &= ~value; return sio_out; }
        if (off == 0x004u) return sio_out;
        if (off == 0x020u) { if (is_write) sio_oe = value; return sio_oe; }
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- STM32H7 RCC: the clock plan a BSP writes, with the ready bits it polls ---- */
    if (in_window(addr, SIM_RCC_BASE)) {
        uint32_t off = addr - SIM_RCC_BASE;
        if (is_write) { rf_set(addr, value); return value; }
        uint32_t v = rf_get(addr);
        if (off == 0x00u) {
            /* CR: HSI is always ready, and PLL1 reports ready as soon as it is asked for.
             * A BSP's bounded poll for "clock ready" therefore terminates, which a model
             * that stored the bit verbatim would turn into a timeout. */
            v |= 0x00000005u;                        /* HSION | HSIRDY */
            if (v & (1u << 24)) v |= (1u << 25);      /* PLL1ON -> PLL1RDY */
        }
        return v;
    }

    /* ---- RP2040: crystal and PLL ---- */
    if (in_window(addr, SIM_RP_XOSC)) {
        uint32_t off = addr - SIM_RP_XOSC;
        if (is_write) { rf_set(addr, value); return value; }
        if (off == 0x04u) return 0x80000000u;         /* STATUS: STABLE */
        return rf_get(addr);
    }
    if (in_window(addr, SIM_RP_PLL_SYS)) {
        uint32_t off = addr - SIM_RP_PLL_SYS;
        if (is_write) { rf_set(addr, value); return value; }
        if (off == 0x00u) {                            /* CS: LOCK once not powered down */
            uint32_t pw = rf_get(SIM_RP_PLL_SYS + 0x04u);
            return (pw & 1u) ? 0u : 0x80000001u;
        }
        return rf_get(addr);
    }

    /* ---- RP2040: UART0 (the telemetry port) ---- */
    if (in_window(addr, SIM_RP_UART0)) {
        uint32_t off = addr - SIM_RP_UART0;
        if (is_write && off < 0x04u) { uart_append((unsigned char)(value & 0xFFu)); return value; }
        if (off == 0x18u) return 0u;                   /* UARTFR: TXFF clear, not busy */
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- RP2040: DMA (one channel carries a telemetry buffer) ---- */
    if (in_window(addr, SIM_RP_DMA)) {
        uint32_t off = addr - SIM_RP_DMA;
        if (off == 0x0Cu && is_write && (value & 1u)) {    /* CTRL_TRIG: enable */
            uint32_t rd = rf_get(SIM_RP_DMA + 0x00u);
            uint32_t wr = rf_get(SIM_RP_DMA + 0x04u);
            uint32_t n = rf_get(SIM_RP_DMA + 0x08u);
            int rd_inc = (int)((value >> 4) & 1u);
            int wr_inc = (int)((value >> 5) & 1u);
            uint32_t sz = (value >> 2) & 3u;
            int size = (sz == 0u) ? 1 : ((sz == 1u) ? 2 : 4);
            for (uint32_t i = 0; i < n; i++) {
                uint32_t src = rd + (rd_inc ? i * (uint32_t)size : 0u);
                uint32_t dst = wr + (wr_inc ? i * (uint32_t)size : 0u);
                uint32_t v;
                if (src >= SIM_RAM_BASE && src < SIM_RAM_BASE + SIM_RAM_SIZE) v = ram_read(src - SIM_RAM_BASE, size);
                else v = kaze_board_mmio(src, size, 0, 0);
                if (dst >= SIM_RAM_BASE && dst < SIM_RAM_BASE + SIM_RAM_SIZE) ram_write(dst - SIM_RAM_BASE, size, v);
                else kaze_board_mmio(dst, size, 1, v);
            }
            rf_set(addr, value & ~1u);
            rf_set(SIM_RP_DMA + 0x40u, 1u);            /* INTR */
            return value;
        }
        if (off == 0x40u && !is_write) return rf_get(addr);
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    /* ---- plain storage inside a peripheral window, or unknown ---- */
    if (in_window(addr, SIM_TIM2_BASE) || in_window(addr, SIM_SPI1_BASE) ||
        in_window(addr, SIM_I2C1_BASE) || in_window(addr, SIM_CAN1_BASE) ||
        in_window(addr, SIM_USART1_BASE) || in_window(addr, SIM_DMA1_BASE) ||
        in_window(addr, SIM_GPIOA_BASE) || in_window(addr, SIM_RCC_BASE) ||
        in_window(addr, SIM_RP_UART0) || in_window(addr, SIM_SYST_CSR) ||
        in_window(addr, SIM_CTL)) {
        if (is_write) { rf_set(addr, value); return value; }
        return rf_get(addr);
    }

    unknown_accesses++;
    if (is_write) { rf_set(addr, value); return value; }
    return rf_get(addr);
}

/* ---- board text ------------------------------------------------------- */
void kaze_board_write(const char* text) {
    if (text == 0) return;
#if SIM_HOST
    fputs(text, stdout);
    fflush(stdout);
#else
    /* lm3s6965evb UART0: data at 0x4000C000, flags at +0x18 (bit 5 set = TX full). */
    for (const char* p = text; *p; p++) {
        while ((*(volatile uint32_t*)0x4000C018u & (1u << 5)) != 0u) { }
        *(volatile uint32_t*)0x4000C000u = (uint32_t)(unsigned char)*p;
    }
#endif
}

/* ---- helpers a suite may want, reachable only through registers ------- */
uint32_t kaze_sim_uart_len(void) { return uart_len; }
const char* kaze_sim_uart_text(void) { return uart_text; }