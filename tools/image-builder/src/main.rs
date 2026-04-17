//! Disk image builder for ClaudioOS — Limine edition.
//!
//! Produces a UEFI-bootable raw disk image that chain-loads the Limine
//! bootloader, which then loads our ELF kernel via the Limine Boot Protocol.
//!
//! Usage:
//!   cargo run --package claudio-image-builder -- <path-to-kernel-elf> [--ramdisk <gguf-path>]
//!
//! Required environment:
//!   LIMINE_DIR — path to a directory containing Limine's prebuilt binaries
//!                (at minimum `BOOTX64.EFI`, typically from the
//!                `limine-bootloader/limine` repo's `v7.x-binary` branch).
//!                If unset, defaults to `./limine` relative to the CWD.
//!
//! Layout produced on the ESP:
//!   /EFI/BOOT/BOOTX64.EFI   -- Limine's UEFI stub (copied from LIMINE_DIR)
//!   /limine.conf            -- bootloader config pointing at kernel.elf
//!   /kernel.elf             -- our ELF kernel
//!   /model.gguf             -- (optional) ramdisk module for local LLM
//!
//! The image is a raw FAT32 filesystem (no GPT). QEMU + OVMF recognize this
//! as a removable-media ESP directly. For real hardware we wrap it in a GPT
//! with a single ESP partition — see `--gpt` flag.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// fatfs auto-selects the FAT variant from volume size; below ~66 MiB it picks
// FAT16, which some bootloaders (Limine v7 UEFI in particular) fail to scan
// for a config file on. Force >=128 MiB so fatfs always picks FAT32.
const FAT32_MIN_BYTES: u64 = 128 * 1024 * 1024;

fn main() {
    let mut args = std::env::args().skip(1);
    let kernel_path = args
        .next()
        .expect("usage: claudio-image-builder <kernel-elf-path> [--ramdisk <gguf-path>]");

    let mut ramdisk: Option<PathBuf> = None;
    let mut make_gpt = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ramdisk" => {
                ramdisk = Some(PathBuf::from(
                    args.next().expect("--ramdisk needs a path"),
                ));
            }
            "--gpt" => make_gpt = true,
            other => eprintln!("warn: unknown arg {:?}", other),
        }
    }

    let kernel_path = Path::new(&kernel_path);
    if !kernel_path.exists() {
        eprintln!("error: kernel ELF not found at {:?}", kernel_path);
        std::process::exit(1);
    }

    let limine_dir = std::env::var("LIMINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("limine"));
    let bootx64 = limine_dir.join("BOOTX64.EFI");
    if !bootx64.exists() {
        eprintln!(
            "error: Limine BOOTX64.EFI not found at {:?}\n\
             set LIMINE_DIR or clone:\n\
             git clone -b v7.x-binary https://github.com/limine-bootloader/limine.git",
            bootx64
        );
        std::process::exit(1);
    }

    let out_dir = kernel_path.parent().unwrap_or(Path::new("."));
    let uefi_path = out_dir.join("claudio-os-uefi.img");

    // ── Size the image large enough to hold kernel + Limine + optional model ──
    let kernel_size = std::fs::metadata(kernel_path).unwrap().len();
    let bootx64_size = std::fs::metadata(&bootx64).unwrap().len();
    let ramdisk_size = ramdisk
        .as_ref()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .unwrap_or(0);

    // Leave 16 MiB headroom for FAT32 structures + limine.conf + slack.
    let content_bytes = kernel_size + bootx64_size + ramdisk_size + 16 * 1024 * 1024;
    let fat_bytes = content_bytes.max(FAT32_MIN_BYTES).next_multiple_of(1024 * 1024);

    println!(
        "[image] kernel={} bytes, limine={} bytes, ramdisk={} bytes, fat_image={} MiB",
        kernel_size,
        bootx64_size,
        ramdisk_size,
        fat_bytes / (1024 * 1024),
    );

    // ── Build the FAT32 filesystem in memory, then write to disk ──────────
    let fat_path = out_dir.join("claudio-os-fat.img");
    make_fat32(&fat_path, fat_bytes, kernel_path, &bootx64, ramdisk.as_deref());

    if make_gpt {
        // Wrap the FAT image in a GPT with a single ESP partition so real
        // hardware firmware recognizes it.
        make_gpt_image(&uefi_path, &fat_path);
        println!("[image] UEFI (GPT) image: {:?}", uefi_path);
    } else {
        // For QEMU: the raw FAT32 is fine as-is.
        std::fs::copy(&fat_path, &uefi_path).expect("copy fat -> uefi");
        println!("[image] UEFI (raw FAT32) image: {:?}", uefi_path);
    }

    println!();
    println!("[image] done! To boot in QEMU:");
    println!();
    println!("  qemu-system-x86_64 \\");
    println!("    -drive if=pflash,format=raw,readonly=on,file=/tmp/ovmf-code.fd \\");
    println!("    -drive format=raw,file={} \\", uefi_path.display());
    println!("    -serial stdio -m 512M -nographic");
}

fn make_fat32(
    path: &Path,
    size: u64,
    kernel: &Path,
    bootx64: &Path,
    ramdisk: Option<&Path>,
) {
    // Create a sparse file of the requested size, format as FAT32, then
    // populate it via the fatfs crate.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("create fat image");
    file.set_len(size).expect("set fat image size");

    fatfs::format_volume(
        &mut fscommon::BufStream::new(&file),
        fatfs::FormatVolumeOptions::new()
            .fat_type(fatfs::FatType::Fat32)
            .volume_label(*b"CLAUDIO    "),
    )
    .expect("format fat32");

    let fs = fatfs::FileSystem::new(
        fscommon::BufStream::new(&file),
        fatfs::FsOptions::new(),
    )
    .expect("open fat32");

    let root = fs.root_dir();
    let efi = root.create_dir("EFI").expect("mkdir EFI");
    let boot = efi.create_dir("BOOT").expect("mkdir EFI/BOOT");

    copy_into(&boot, "BOOTX64.EFI", bootx64);
    copy_into(&root, "kernel.elf", kernel);

    // Write limine.cfg (v6 syntax — the shipped v7.x-binary BOOTX64.EFI only
    // recognizes `limine.cfg` + uppercase KEY=VALUE tokens. `limine.conf` /
    // `key: value` is v7.12+ and is not present in 7.13.3 binaries.)
    let mut conf = String::new();
    conf.push_str("TIMEOUT=0\n");
    conf.push_str("DEFAULT_ENTRY=1\n");
    conf.push_str("SERIAL=yes\n");
    conf.push_str("VERBOSE=yes\n\n");
    conf.push_str(":ClaudioOS\n");
    conf.push_str("PROTOCOL=limine\n");
    conf.push_str("KERNEL_PATH=boot:///kernel.elf\n");
    if ramdisk.is_some() {
        conf.push_str("MODULE_PATH=boot:///model.gguf\n");
        conf.push_str("MODULE_CMDLINE=model.gguf\n");
    }
    // Write limine.conf at all canonical search paths so firmware-picky
    // setups find it: / , /boot/ , /EFI/BOOT/
    for target in [
        ("limine.cfg", None),
        ("limine.cfg", Some(&boot)),
    ] {
        let (name, dir) = target;
        let parent = dir.unwrap_or(&root);
        let mut f = parent.create_file(name).expect("create limine.conf");
        f.write_all(conf.as_bytes()).expect("write limine.conf");
    }
    let boot_dir = root.create_dir("boot").expect("mkdir /boot");
    let mut f = boot_dir.create_file("limine.cfg").expect("create /boot/limine.conf");
    f.write_all(conf.as_bytes()).expect("write /boot/limine.conf");

    if let Some(rd) = ramdisk {
        copy_into(&root, "model.gguf", rd);
    }

    println!("[image] FAT32 populated: {:?}", path);
}

fn copy_into<IO>(dir: &fatfs::Dir<IO>, name: &str, src: &Path)
where
    IO: fatfs::ReadWriteSeek,
{
    let mut f = dir.create_file(name).unwrap_or_else(|e| {
        panic!("create {:?} in fat: {:?}", name, e);
    });
    let mut src_f = std::fs::File::open(src).expect("open src");
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src_f.read(&mut buf).expect("read src");
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).expect("write fat");
    }
    println!("[image]   + {}", name);
}

fn make_gpt_image(out: &Path, fat: &Path) {
    use gpt::{partition_types, GptConfig, GptDisk};

    let fat_len = std::fs::metadata(fat).unwrap().len();
    // 1 MiB padding at start (GPT header + partition table) + 1 MiB at end.
    let total = fat_len + 2 * 1024 * 1024;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(out)
        .expect("create gpt image");
    file.set_len(total).expect("set gpt size");

    let mut disk: GptDisk = GptConfig::new()
        .writable(true)
        .initialized(false)
        .create_from_device(Box::new(file), None)
        .expect("gpt init");
    disk.update_partitions(Default::default()).unwrap();
    let _part_id = disk
        .add_partition(
            "ESP",
            fat_len,
            partition_types::EFI,
            0,
            None,
        )
        .expect("add ESP partition");
    let _ = disk.write().expect("write gpt");

    // Now write the FAT contents into the partition area. The partition
    // starts at the first LBA after the GPT header + partition table —
    // typically LBA 2048 (1 MiB).
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(out)
        .unwrap();
    f.seek(SeekFrom::Start(1024 * 1024)).unwrap();
    let mut fat_f = std::fs::File::open(fat).unwrap();
    std::io::copy(&mut fat_f, &mut f).unwrap();
}

// Minimal BufStream vendored locally — fatfs 0.3 needs it and the
// `fscommon` crate ships a compatible implementation. Pull it in lazily.
mod fscommon {
    use std::io::{Read, Seek, SeekFrom, Write};

    pub struct BufStream<T> {
        inner: T,
    }
    impl<T: Read + Write + Seek> BufStream<T> {
        pub fn new(inner: T) -> Self {
            Self { inner }
        }
    }
    impl<T: Read> Read for BufStream<T> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }
    }
    impl<T: Write> Write for BufStream<T> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    impl<T: Seek> Seek for BufStream<T> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }
}
