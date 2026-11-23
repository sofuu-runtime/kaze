.syntax unified
.thumb
.section .text.start
.global _start
/* Cortex-M boots by loading SP from 0x0 and PC from 0x4, so the first two words of the
.   image are the vector table. The reset vector must have bit 0 set: a Cortex-M is
.   Thumb-only, and a clear bit 0 is interpreted as ARM state, which locks up before the
.   first instruction. `.thumb_func` is what makes the assembler and linker set it. */
.word __stack_top
.word _start
.thumb_func
_start:
    ldr sp, =__stack_top
    bl main
    b .