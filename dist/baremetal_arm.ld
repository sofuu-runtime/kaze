/* Minimal linker script for lm3s6965evb: code at 0x0, data and stack in RAM. */
MEMORY { FLASH (rx) : ORIGIN = 0x00000000, LENGTH = 256K
         RAM  (rwx) : ORIGIN = 0x20000000, LENGTH = 64K }
ENTRY(_start)
SECTIONS {
  .text : { KEEP(*(.text.start)) *(.text*) *(.rodata*) } > FLASH
  .data : { *(.data*) } > RAM AT > FLASH
  .bss  : { *(.bss*) *(COMMON) } > RAM
  __stack_top = ORIGIN(RAM) + LENGTH(RAM);
}
