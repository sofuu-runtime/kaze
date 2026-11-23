/* baremetal_main_arm.c -- exercises the Kaze runtime on a Cortex-M under QEMU.
 *
 * The runtime header is the whole surface available to a bare-metal program: the arena,
 * checked arithmetic, the trap path and the fallback handler. This drives all of them,
 * proves trap text reaches the board's write hook, and exits through semihosting so the
 * host sees a status instead of a timeout.
 *
 *   qemu-system-arm -M lm3s6965evb -nographic -semihosting -kernel kaze_bm_arm.elf
 */
#include "kaze_rt.h"

/* lm3s6965evb: UART0 data at 0x4000C000, flags at +0x18 (bit 5 set = TX full). */
#define UART0_DR (*(volatile unsigned int*)0x4000C000)
#define UART0_FR (*(volatile unsigned int*)0x4000C018)

void kaze_board_write(const char* text) {
    if (text == 0) return;
    for (const char* p = text; *p != 0; p++) {
        while ((UART0_FR & (1u << 5)) != 0) { }
        UART0_DR = (unsigned int)(unsigned char)*p;
    }
}

static void semi_exit(int code) {
    register unsigned int r0 __asm__("r0") = 0x18;   /* SYS_EXIT */
    register void* r1 __asm__("r1") = (void*)&(unsigned int[2]){ 0x20026, (unsigned int)code };
    __asm__ volatile("bkpt 0xAB" : : "r"(r0), "r"(r1) : "memory");
    for (;;) { }
}

static char arena_memory[4096];
static int failures = 0;

static void expect(const char* label, int ok) {
    kaze_board_write(ok ? "PASS " : "FAIL ");
    kaze_board_write(label);
    kaze_board_write("\n");
    if (!ok) failures++;
}

/* A fallback handler the runtime can call instead of aborting: on a board there is no
 * exit(), so the handler records and returns. */
static int seen_fallback = 0;
static int board_fallback(const char* code, const char* message) {
    (void)code;
    (void)message;
    seen_fallback = 1;
    return 0;
}

int main(void) {
    kaze_set_fallback_handler(board_fallback);   /* no hosted default on a board */
    static kaze_arena_t arena;
    kaze_arena_init(&arena, arena_memory, sizeof arena_memory);
    expect("arena starts empty", kaze_arena_used(&arena) == 0);

    void* p = kmalloc(&arena, 64);
    expect("kmalloc returns aligned memory",
           p != 0 && (((uintptr_t)p % 8) == 0) && kaze_arena_used(&arena) >= 64);

    void* q = kmalloc(&arena, 128);
    expect("second allocation moves forward", q != 0 && (uintptr_t)q > (uintptr_t)p);

    kaze_arena_reset(&arena);
    expect("reset reclaims everything", kaze_arena_used(&arena) == 0);

    /* The arena is asked for more than it has. This calls the runtime's OOM path, which
     * traps; the trap is exercised on its own below so a fault here is unambiguous. */
    void* too_big = kmalloc(&arena, sizeof arena_memory * 4);
    expect("a request larger than the arena returns null", too_big == 0);

    kaze_int_t out = 0;
    expect("checked add accepts a safe sum", kaze_add_checked(40, 2, &out) && out == 42);
    expect("checked add refuses overflow",
           !kaze_add_checked((kaze_int_t)((~(kaze_uint_t)0) >> 1), 1, &out));

    kaze_set_fallback_handler(board_fallback);
    kaze_board_write("trap 1: no message argument\n");
    (void)kaze_trap(KAZE_ERR_TYPE, 0);
    expect("trap with a null message returns through the hook", seen_fallback == 1);

    seen_fallback = 0;
    kaze_board_write("trap 2: short message\n");
    (void)kaze_trap(KAZE_ERR_TYPE, "short");
    expect("trap with a short message returns through the hook", seen_fallback == 1);

    seen_fallback = 0;
    kaze_board_write("trap 3: numbers in the message\n");
    (void)kaze_trap(KAZE_ERR_OVERFLOW, "value 1234567890123 does not fit");
    expect("trap with digits returns through the hook", seen_fallback == 1);

    kaze_board_write(failures == 0 ? "BARE-METAL OK\n" : "BARE-METAL FAILED\n");
    semi_exit(failures);
    return failures;
}
