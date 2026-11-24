.syntax unified
.thumb
.section .text.start
.global _start
/* Cortex-M boots by loading SP from 0x0 and PC from 0x4, so the first two words of the
   image are the vector table. The reset vector must have bit 0 set: a Cortex-M is
   Thumb-only, and a clear bit 0 is interpreted as ARM state, which locks up before the
   first instruction. '.thumb_func' is what makes the assembler and linker set it.

   .data is copied from flash and .bss is zeroed before main, because a section that runs
   in RAM but is stored in flash is not there until something puts it there. Without this
   the images happened to work (their .data was pointer constants the linker had already
   resolved) while any initialised static in a driver would have read as zero.

   main takes (argc, argv) and a board has neither, so both registers are cleared. When main
   returns, its status goes to the debugger through semihosting SYS_EXIT: there is no
   operating system to return to, and a green QEMU run has to mean the program finished. */
.word __stack_top
.word _start
.thumb_func
_start:
    ldr sp, =__stack_top

    ldr r0, =__data_start
    ldr r1, =__data_load_start
    ldr r2, =__data_end
1:  cmp r0, r2
    bhs 2f
    ldr r3, [r1], #4
    str r3, [r0], #4
    b 1b
2:
    ldr r0, =__bss_start
    ldr r1, =__bss_end
    movs r3, #0
3:  cmp r0, r1
    bhs 4f
    str r3, [r0], #4
    b 3b
4:
    movs r0, #0
    movs r1, #0
    bl main

    sub sp, #8
    ldr r2, =0x20026
    str r2, [sp]
    str r0, [sp, #4]
    mov r1, sp
    movs r0, #0x18
    bkpt 0xAB
5:  b 5b
