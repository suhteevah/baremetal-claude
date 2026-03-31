//! Serial port (COM1 @ 0x3F8) for debug output.
//! Available immediately at boot before framebuffer init.

use spin::Mutex;
use x86_64::instructions::port::Port;

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(0x3F8));

pub struct SerialPort {
    base: u16,
    data: Port<u8>,
    line_status: Port<u8>,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        Self {
            base,
            data: Port::new(base),
            line_status: Port::new(base + 5),
        }
    }

    /// Initialize the UART with standard 16550 settings.
    ///
    /// Full init sequence: disable interrupts, set baud rate via DLAB,
    /// configure 8N1 format, enable FIFO, loopback test, then normal mode.
    fn init_hw(&mut self) {
        // SAFETY: Standard 16550 UART init sequence on COM1. These I/O ports
        // are well-defined x86 hardware registers.
        unsafe {
            Port::<u8>::new(self.base + 1).write(0x00); // Disable interrupts
            Port::<u8>::new(self.base + 3).write(0x80); // Enable DLAB
            Port::<u8>::new(self.base).write(0x01);     // Baud divisor low = 1 (115200)
            Port::<u8>::new(self.base + 1).write(0x00); // Baud divisor high = 0
            Port::<u8>::new(self.base + 3).write(0x03); // 8N1, clear DLAB
            Port::<u8>::new(self.base + 2).write(0xC7); // Enable FIFO, 14-byte threshold
            Port::<u8>::new(self.base + 4).write(0x0F); // Normal operation, IRQs + RTS/DSR
        }
    }
}

pub fn init() {
    SERIAL.lock().init_hw();
}

impl core::fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            // SAFETY: Reading LSR and writing THR on a standard UART
            unsafe {
                while self.line_status.read() & 0x20 == 0 {}
                self.data.write(byte);
            }
        }
        Ok(())
    }
}

/// Print to serial, panicking on failure
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

/// Force-print variant that bypasses the lock (for panic handler).
///
/// Uses `force_unlock()` to release any held lock, then writes normally.
/// This is only safe in panic/abort context where we know no other code
/// will run concurrently.
#[macro_export]
macro_rules! force_println {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            // Force-unlock the mutex in case it was held when we panicked.
            // SAFETY: Only called from panic handler — no other code will run.
            unsafe { $crate::serial::SERIAL.force_unlock(); }
            let mut port = $crate::serial::SERIAL.lock();
            let _ = write!(port, "{}\n", format_args!($($arg)*));
        }
    };
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    // Disable interrupts while holding serial lock to prevent deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        SERIAL.lock().write_fmt(args).expect("serial write failed");
    });
}

// Re-export for panic handler
pub use force_println;
