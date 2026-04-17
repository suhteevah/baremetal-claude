/* ClaudioOS kernel linker script for Limine Boot Protocol
 *
 * Maps the kernel at the higher-half canonical address 0xffffffff80000000.
 * Mirrors the layout used by the limine-rust-template.
 *
 * Sections:
 *   .text     — executable code
 *   .rodata   — read-only data (Limine requests live here inside
 *               `.requests` / `.requests_start_marker` / `.requests_end_marker`
 *               so the bootloader can find them by scanning our ELF).
 *   .data     — writable initialized data
 *   .bss      — zero-initialized data
 *
 * The requests markers are required by the Limine protocol so the bootloader
 * can locate the request list without scanning the full kernel image.
 */

OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS
{
    limine_requests PT_LOAD;
    text            PT_LOAD;
    rodata          PT_LOAD;
    data            PT_LOAD;
}

SECTIONS
{
    /* Kernel lives in the topmost 2 GiB of the canonical virtual address space
     * (the higher half). This is the address Limine expects for the "limine"
     * protocol with the default paging mode on x86_64. */
    . = 0xffffffff80000000;

    /* Limine requests — placed first so the bootloader can locate them cheaply.
     * The start/end markers bracket all request structs in `.requests`. */
    .requests : {
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :limine_requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.interp)
    }
}
