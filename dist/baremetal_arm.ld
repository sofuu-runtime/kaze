/* Minimal linker script for lm3s6965evb: code at 0x0, data and stack in RAM.
 *
 * The stack is a reserved region at the top of RAM rather than "whatever .bss did not use":
 * a deep call chain that ran past the end of .bss would otherwise corrupt it silently, and
 * that is not a failure mode worth leaving in the image the timing gates run on. If code
 * plus data plus the 8 KB stack does not fit, the link now fails instead of misbehaving. */
MEMORY { FLASH (rx) : ORIGIN = 0x00000000, LENGTH = 256K
         RAM  (rwx) : ORIGIN = 0x20000000, LENGTH = 64K }
ENTRY(_start)
SECTIONS {
  .text : {
    KEEP(*(.text.start))
    *(.text*)
    *(.rodata*)
    . = ALIGN(4);
  } > FLASH

  .data : {
    . = ALIGN(4);
    __data_start = .;
    *(.data*)
    . = ALIGN(4);
    __data_end = .;
  } > RAM AT > FLASH
  __data_load_start = LOADADDR(.data);

  .bss (NOLOAD) : {
    . = ALIGN(4);
    __bss_start = .;
    *(.bss*)
    *(COMMON)
    . = ALIGN(4);
    __bss_end = .;
  } > RAM

  .stack (NOLOAD) : {
    . = ALIGN(8);
    . = . + 8192;
    __stack_top = .;
  } > RAM
}
