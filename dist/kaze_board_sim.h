/* kaze_board_sim.h -- the virtual board the Phase 14 drivers are exercised against.
 *
 * Two things need the same register map: the host suites (fast, under ASan) and the
 * QEMU image (real ARM instructions, so the timing gates measure something). Both link
 * this file. The Kaze BSPs carry the same numbers as constants, which is why every access
 * is checked: kaze_board_mmio counts reads and writes to addresses no model claims, and
 * the suites assert that count is zero. A BSP that drifts from this map fails loudly
 * instead of quietly reading zero.
 *
 * Nothing here is silicon. The peripheral models are small and behavioural: a timer that
 * counts, an SPI bus with an IMU on it, an I2C barometer, a CAN controller in loopback,
 * a DMA channel, a UART that keeps what it was given.
 *
 *   KAZE_SIM_METAL   built for a bare-metal image: SysTick is the real thing (so time is
 *                    instruction-accurate under QEMU -icount) and output goes to the
 *                    board UART instead of stdout.
 */
#ifndef KAZE_BOARD_SIM_H
#define KAZE_BOARD_SIM_H

#include <stdint.h>

/* ---- STM32H7-style peripherals ---------------------------------------- */
#define SIM_TIM2_BASE     0x40000000u   /* CR1 0x00 DIER 0x0C SR 0x10 EGR 0x14 CCMR1 0x18
                                         * CCMR2 0x1C CCER 0x20 CNT 0x24 PSC 0x28
                                         * ARR 0x2C CCR1 0x34 CCR2 0x38 CCR3 0x3C
                                         * CCR4 0x40 BDTR 0x44 */
#define SIM_I2C1_BASE     0x40005400u   /* CR1 0x00 CR2 0x04 OAR1 0x08 TIMINGR 0x10
                                         * ISR 0x18 ICR 0x1C RXDR 0x24 TXDR 0x28 */
#define SIM_CAN1_BASE     0x4000A400u   /* MCR 0x00 MSR 0x04 TSR 0x08 RF0R 0x0C
                                         * TIR 0x180 TDTR 0x184 TDLR 0x188 TDHR 0x18C
                                         * RIR 0x1B0 RDTR 0x1B4 RDLR 0x1B8 RDHR 0x1BC */
#define SIM_USART1_BASE   0x40011000u   /* CR1 0x00 BRR 0x0C ISR 0x1C RDR 0x24 TDR 0x28 */
#define SIM_SPI1_BASE     0x40013000u   /* CR1 0x00 CR2 0x04 SR 0x08 DR 0x0C */
#define SIM_DMA1_BASE     0x40020000u   /* LISR 0x00 S0CR 0x10 S0NDTR 0x14 S0PAR 0x18
                                         * S0M0AR 0x1C S1CR 0x28 ... */
#define SIM_GPIOA_BASE    0x58020000u   /* MODER 0x00 OTYPER 0x04 OSPEEDR 0x08
                                         * PUPDR 0x0C IDR 0x10 ODR 0x14 BSRR 0x18
                                         * AFRL 0x20 AFRH 0x24 */

/* ---- RP2040-style peripherals ----------------------------------------- */
#define SIM_RP_RESETS     0x4000C000u   /* RESET 0x00 WDSEL 0x04 RESET_DONE 0x08 */
#define SIM_RP_CLOCKS     0x40008000u   /* CLK_REF_CTRL 0x30 CLK_SYS_CTRL 0x3C */
#define SIM_RP_IO_BANK0   0x40014000u   /* GPIO0_CTRL 0x04 + n*0x08 */
#define SIM_RP_PADS       0x4001C000u   /* GPIO0 0x04 + n*0x04 */
#define SIM_RP_SPI0       0x4003C000u   /* SSPCR0 0x00 SSPCR1 0x04 SSPDR 0x08 SSPSR 0x0C */
#define SIM_RP_I2C0       0x40044000u   /* IC_CON 0x00 IC_TAR 0x04 IC_DATA_CMD 0x10
                                         * IC_RAW_INTR_STAT 0x34 IC_STATUS 0x70 */
#define SIM_RP_PWM        0x40050000u   /* CHn_CSR 0x00+n*0x14 CHn_DIV +4 CHn_CTR +8
                                         * CHn_CC +12 CHn_TOP +16  EN 0xF0  INTR 0xF4 */
#define SIM_RP_TIMER      0x40054000u   /* TIMEHR 0x08 TIMELR 0x0C ALARM0 0x10
                                         * TIMERAWL 0x28 */
#define SIM_RP_SIO        0xD0000000u   /* GPIO_IN 0x004 GPIO_OUT 0x010
                                         * GPIO_OUT_SET 0x014 GPIO_OUT_CLR 0x018
                                         * GPIO_OE 0x020 */

/* ---- STM32H7 RCC (so a BSP's clock plan can be written and read back) - */
#define SIM_RCC_BASE      0x58024400u   /* CR 0x00 CFGR 0x10 D1CFGR 0x14 D2CFGR 0x18
                                         * D3CFGR 0x1C PLLCKSELR 0x28 PLLCFGR 0x30
                                         * PLL1DIVR 0x34 PLL1FRACR 0x38 */

/* ---- RP2040 clock sources and the two windows the BSP record names -- */
#define SIM_RP_XOSC       0x40024000u   /* CTRL 0x00 STATUS 0x04 */
#define SIM_RP_PLL_SYS    0x40028000u   /* CS 0x00 PWR 0x04 FBDIV_INT 0x08 PRIM 0x0C */
#define SIM_RP_UART0      0x40034000u   /* UARTDR 0x00 UARTFR 0x18 (bit5 TXFF)
                                         * UARTIBRD 0x24 UARTFBRD 0x28 UARTLCR_H 0x2C
                                         * UARTCR 0x30 */
#define SIM_RP_DMA        0x50000000u   /* CH0_READ_ADDR 0x00 CH0_WRITE_ADDR 0x04
                                         * CH0_TRANS_COUNT 0x08 CH0_CTRL_TRIG 0x0C
                                         * INTR 0x40 */

/* ---- RAM the DMA model can move bytes through -------------------------- */
#define SIM_RAM_BASE      0x24000000u
#define SIM_RAM_SIZE      0x1000u

/* ---- test-only control block (no silicon counterpart) ----------------- */
#define SIM_CTL           0xE00F0000u
#define SIM_CTL_CYCLES        0x00u   /* free-running model clock, in core cycles */
#define SIM_CTL_EDGE_COUNT    0x04u   /* actuator edges logged so far */
#define SIM_CTL_EDGE_INDEX    0x08u   /* write k, then read EDGE_TIME / EDGE_VALUE */
#define SIM_CTL_EDGE_TIME     0x0Cu   /* cycle stamp of logged edge k */
#define SIM_CTL_EDGE_VALUE    0x10u   /* register value written at logged edge k */
#define SIM_CTL_UNKNOWN       0x14u   /* accesses to addresses no model claims */
#define SIM_CTL_BAD_ALIGN     0x18u   /* misaligned access count */
#define SIM_CTL_UART_LEN      0x1Cu   /* bytes the UART was given */
#define SIM_CTL_UART_INDEX    0x20u   /* write k, then read UART_BYTE */
#define SIM_CTL_UART_BYTE     0x24u
#define SIM_CTL_IMU_SAMPLES   0x28u   /* IMU conversions completed */
#define SIM_CTL_CAN_RX        0x2Cu   /* frames the CAN model accepted */
#define SIM_CTL_RESET         0x30u   /* write 1 to clear the register file */
#define SIM_CTL_SYSCLK_HZ     0x34u   /* the clock the BSP thinks it configured */
#define SIM_CTL_SYSTICK_REAL  0x38u   /* 1 when SysTick is the emulator's, not ours */

/* SysTick, which is emulated by QEMU (instruction-accurate under -icount) rather than
 * modelled here, on the metal build. */
#define SIM_SYST_CSR      0xE000E010u
#define SIM_SYST_RVR      0xE000E014u
#define SIM_SYST_CVR      0xE000E018u
#define SIM_SYST_CALIB    0xE000E01Cu

/* The one hook a Kaze program can reach: width is 1, 2 or 4 bytes, is_write selects the
 * direction, and the return value is the register (or memory) contents for a read. */
uint32_t kaze_board_mmio(uint32_t addr, int width, int is_write, uint32_t value);

/* Board text sink: trap messages and anything a program sends. */
void kaze_board_write(const char* text);

/* Model clock in core cycles. On metal this is SysTick; on the host it advances by a
 * fixed cost per register access, because a host suite asserts behaviour, not timing. */
uint32_t kaze_sim_cycles(void);

#endif /* KAZE_BOARD_SIM_H */
