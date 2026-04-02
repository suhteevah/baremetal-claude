//! Built-in shell commands.
//!
//! All commands operate on VFS paths. Each command parses args, validates,
//! executes, and returns an output string.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::env::Environment;

/// Trait abstracting filesystem operations so builtins work against any VFS backend.
pub trait Vfs {
    /// List directory entries at `path`. Returns names.
    fn list_dir(&self, path: &str) -> Result<Vec<String>, String>;
    /// Read a file's contents as bytes.
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String>;
    /// Write bytes to a file (create or overwrite).
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
    /// Append bytes to a file.
    fn append_file(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
    /// Copy a file from src to dst.
    fn copy_file(&mut self, src: &str, dst: &str) -> Result<(), String>;
    /// Move/rename a file from src to dst.
    fn move_file(&mut self, src: &str, dst: &str) -> Result<(), String>;
    /// Remove a file or directory.
    fn remove(&mut self, path: &str) -> Result<(), String>;
    /// Create a directory.
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    /// Create an empty file (or update timestamp).
    fn touch(&mut self, path: &str) -> Result<(), String>;
    /// Check if a path exists.
    fn exists(&self, path: &str) -> bool;
    /// Check if a path is a directory.
    fn is_dir(&self, path: &str) -> bool;
    /// Mount a device at a path with the given fstype.
    fn mount(&mut self, device: &str, path: &str, fstype: &str) -> Result<(), String>;
    /// Unmount a filesystem at path.
    fn umount(&mut self, path: &str) -> Result<(), String>;
}

/// Trait abstracting system queries (agent sessions, memory, network, etc.).
pub trait SystemInfo {
    /// List active agent sessions: (id, name, status).
    fn list_agents(&self) -> Vec<(u64, String, String)>;
    /// Kill an agent session by ID.
    fn kill_agent(&mut self, id: u64) -> Result<(), String>;
    /// Clear the terminal screen.
    fn clear_screen(&mut self);
    /// Reboot the system.
    fn reboot(&mut self) -> !;
    /// Shutdown the system.
    fn shutdown(&mut self) -> !;
    /// Get network interface info: (name, ip, mac, status).
    fn ifconfig(&self) -> Vec<(String, String, String, String)>;
    /// Ping a host, return round-trip time or error.
    fn ping(&self, host: &str) -> Result<String, String>;
    /// Get current date/time as a formatted string.
    fn date(&self) -> String;
    /// Get uptime in seconds.
    fn uptime_secs(&self) -> u64;
    /// Get memory info: (total_bytes, used_bytes, free_bytes).
    fn memory_info(&self) -> (u64, u64, u64);
    /// Get disk usage: Vec<(mount_point, total_bytes, used_bytes, available_bytes)>.
    fn disk_usage(&self) -> Vec<(String, u64, u64, u64)>;
}

/// Execute a built-in command, returning the output string.
/// Returns `None` if the command name is not a recognized builtin.
pub fn execute_builtin(
    name: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    env: &mut Environment,
    vfs: &mut dyn Vfs,
    sys: &mut dyn SystemInfo,
) -> Option<Result<String, String>> {
    log::debug!(
        "shell::builtin: checking command {:?} with {} arg(s)",
        name,
        args.len()
    );

    let result = match name {
        "ls" => Some(cmd_ls(args, env, vfs)),
        "cd" => Some(cmd_cd(args, env, vfs)),
        "pwd" => Some(cmd_pwd(env)),
        "cat" => Some(cmd_cat(args, stdin, env, vfs)),
        "echo" => Some(cmd_echo(args)),
        "cp" => Some(cmd_cp(args, env, vfs)),
        "mv" => Some(cmd_mv(args, env, vfs)),
        "rm" => Some(cmd_rm(args, env, vfs)),
        "mkdir" => Some(cmd_mkdir(args, env, vfs)),
        "touch" => Some(cmd_touch(args, env, vfs)),
        "head" => Some(cmd_head(args, stdin, env, vfs)),
        "tail" => Some(cmd_tail(args, stdin, env, vfs)),
        "grep" => Some(cmd_grep(args, stdin, env, vfs)),
        "mount" => Some(cmd_mount(args, vfs)),
        "umount" => Some(cmd_umount(args, vfs)),
        "ps" => Some(cmd_ps(sys)),
        "kill" => Some(cmd_kill(args, sys)),
        "clear" => Some(cmd_clear(sys)),
        "help" => Some(cmd_help()),
        "reboot" => cmd_reboot(sys),
        "shutdown" => cmd_shutdown(sys),
        "ifconfig" => Some(cmd_ifconfig(sys)),
        "ping" => Some(cmd_ping(args, sys)),
        "ssh" => Some(cmd_ssh(args)),
        "date" => Some(cmd_date(sys)),
        "uptime" => Some(cmd_uptime(sys)),
        "free" => Some(cmd_free(sys)),
        "df" => Some(cmd_df(sys)),
        "set" => Some(cmd_set(args, env)),
        "unset" => Some(cmd_unset(args, env)),
        "export" => Some(cmd_export(args, env)),
        "history" => Some(Ok(String::from("[history shown by shell loop]"))),
        "exit" => Some(Ok(String::from("exit"))),
        _ => {
            log::trace!("shell::builtin: {:?} is not a builtin", name);
            None
        }
    };

    if let Some(ref r) = result {
        match r {
            Ok(out) => log::debug!(
                "shell::builtin: {:?} completed successfully ({} bytes output)",
                name,
                out.len()
            ),
            Err(e) => log::warn!("shell::builtin: {:?} failed: {}", name, e),
        }
    }

    result
}

/// Resolve a path relative to PWD if it's not absolute.
fn resolve_path(path: &str, env: &Environment) -> String {
    if path.starts_with('/') {
        String::from(path)
    } else {
        let pwd = env.pwd();
        if pwd.ends_with('/') {
            format!("{}{}", pwd, path)
        } else {
            format!("{}/{}", pwd, path)
        }
    }
}

// --- Individual command implementations ---

fn cmd_ls(args: &[String], env: &Environment, vfs: &dyn Vfs) -> Result<String, String> {
    let path = if args.is_empty() {
        env.pwd().into()
    } else {
        resolve_path(&args[0], env)
    };

    log::info!("shell::builtin::ls: listing {:?}", path);

    let entries = vfs.list_dir(&path)?;

    if entries.is_empty() {
        Ok(String::new())
    } else {
        Ok(entries.join("\n"))
    }
}

fn cmd_cd(args: &[String], env: &mut Environment, vfs: &dyn Vfs) -> Result<String, String> {
    let target = if args.is_empty() {
        String::from(env.home())
    } else {
        resolve_path(&args[0], env)
    };

    log::info!("shell::builtin::cd: changing to {:?}", target);

    if !vfs.exists(&target) {
        return Err(format!("cd: {}: No such file or directory", target));
    }
    if !vfs.is_dir(&target) {
        return Err(format!("cd: {}: Not a directory", target));
    }

    env.set_pwd(&target);
    Ok(String::new())
}

fn cmd_pwd(env: &Environment) -> Result<String, String> {
    let pwd = env.pwd();
    log::info!("shell::builtin::pwd: {}", pwd);
    Ok(String::from(pwd))
}

fn cmd_cat(
    args: &[String],
    stdin: Option<&[u8]>,
    env: &Environment,
    vfs: &dyn Vfs,
) -> Result<String, String> {
    if args.is_empty() {
        // Read from stdin if piped
        if let Some(data) = stdin {
            log::info!("shell::builtin::cat: reading from stdin ({} bytes)", data.len());
            return Ok(String::from_utf8_lossy(data).into_owned());
        }
        return Err(String::from("cat: missing file operand"));
    }

    log::info!("shell::builtin::cat: reading {} file(s)", args.len());

    let mut output = String::new();
    for arg in args {
        let path = resolve_path(arg, env);
        log::debug!("shell::builtin::cat: reading {:?}", path);
        let data = vfs.read_file(&path)?;
        output.push_str(&String::from_utf8_lossy(&data));
    }

    Ok(output)
}

fn cmd_echo(args: &[String]) -> Result<String, String> {
    let output = args.join(" ");
    log::info!("shell::builtin::echo: {:?}", output);
    Ok(output)
}

fn cmd_cp(args: &[String], env: &Environment, vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.len() < 2 {
        return Err(String::from("cp: missing operand"));
    }
    let src = resolve_path(&args[0], env);
    let dst = resolve_path(&args[1], env);
    log::info!("shell::builtin::cp: {:?} -> {:?}", src, dst);
    vfs.copy_file(&src, &dst)?;
    Ok(String::new())
}

fn cmd_mv(args: &[String], env: &Environment, vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.len() < 2 {
        return Err(String::from("mv: missing operand"));
    }
    let src = resolve_path(&args[0], env);
    let dst = resolve_path(&args[1], env);
    log::info!("shell::builtin::mv: {:?} -> {:?}", src, dst);
    vfs.move_file(&src, &dst)?;
    Ok(String::new())
}

fn cmd_rm(args: &[String], env: &Environment, vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("rm: missing operand"));
    }
    for arg in args {
        let path = resolve_path(arg, env);
        log::info!("shell::builtin::rm: removing {:?}", path);
        vfs.remove(&path)?;
    }
    Ok(String::new())
}

fn cmd_mkdir(args: &[String], env: &Environment, vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("mkdir: missing operand"));
    }
    for arg in args {
        let path = resolve_path(arg, env);
        log::info!("shell::builtin::mkdir: creating {:?}", path);
        vfs.mkdir(&path)?;
    }
    Ok(String::new())
}

fn cmd_touch(args: &[String], env: &Environment, vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("touch: missing operand"));
    }
    for arg in args {
        let path = resolve_path(arg, env);
        log::info!("shell::builtin::touch: {:?}", path);
        vfs.touch(&path)?;
    }
    Ok(String::new())
}

fn cmd_head(
    args: &[String],
    stdin: Option<&[u8]>,
    env: &Environment,
    vfs: &dyn Vfs,
) -> Result<String, String> {
    let (file_arg, n) = parse_line_count_args(args, 10);

    let content = if let Some(path) = file_arg {
        let resolved = resolve_path(path, env);
        log::info!("shell::builtin::head: {:?} (n={})", resolved, n);
        let data = vfs.read_file(&resolved)?;
        String::from_utf8_lossy(&data).into_owned()
    } else if let Some(data) = stdin {
        log::info!("shell::builtin::head: stdin (n={})", n);
        String::from_utf8_lossy(data).into_owned()
    } else {
        return Err(String::from("head: missing file operand"));
    };

    let lines: Vec<&str> = content.lines().take(n).collect();
    Ok(lines.join("\n"))
}

fn cmd_tail(
    args: &[String],
    stdin: Option<&[u8]>,
    env: &Environment,
    vfs: &dyn Vfs,
) -> Result<String, String> {
    let (file_arg, n) = parse_line_count_args(args, 10);

    let content = if let Some(path) = file_arg {
        let resolved = resolve_path(path, env);
        log::info!("shell::builtin::tail: {:?} (n={})", resolved, n);
        let data = vfs.read_file(&resolved)?;
        String::from_utf8_lossy(&data).into_owned()
    } else if let Some(data) = stdin {
        log::info!("shell::builtin::tail: stdin (n={})", n);
        String::from_utf8_lossy(data).into_owned()
    } else {
        return Err(String::from("tail: missing file operand"));
    };

    let all_lines: Vec<&str> = content.lines().collect();
    let start = if all_lines.len() > n {
        all_lines.len() - n
    } else {
        0
    };
    let lines = &all_lines[start..];
    Ok(lines.join("\n"))
}

/// Parse args for head/tail: optional -n NUM and a file path.
fn parse_line_count_args(args: &[String], default_n: usize) -> (Option<&str>, usize) {
    let mut n = default_n;
    let mut file: Option<&str> = None;
    let mut i = 0;

    while i < args.len() {
        if args[i] == "-n" && i + 1 < args.len() {
            if let Ok(count) = args[i + 1].parse::<usize>() {
                n = count;
            }
            i += 2;
        } else if args[i].starts_with('-') && args[i].len() > 1 {
            // -NUM shorthand
            if let Ok(count) = args[i][1..].parse::<usize>() {
                n = count;
            }
            i += 1;
        } else {
            file = Some(args[i].as_str());
            i += 1;
        }
    }

    (file, n)
}

fn cmd_grep(
    args: &[String],
    stdin: Option<&[u8]>,
    env: &Environment,
    vfs: &dyn Vfs,
) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("grep: missing pattern"));
    }

    let pattern = &args[0];
    log::info!("shell::builtin::grep: pattern={:?}", pattern);

    let content = if args.len() >= 2 {
        let path = resolve_path(&args[1], env);
        log::debug!("shell::builtin::grep: searching in {:?}", path);
        let data = vfs.read_file(&path)?;
        String::from_utf8_lossy(&data).into_owned()
    } else if let Some(data) = stdin {
        log::debug!("shell::builtin::grep: searching in stdin");
        String::from_utf8_lossy(data).into_owned()
    } else {
        return Err(String::from("grep: missing file operand"));
    };

    // Simple substring matching (no regex engine in no_std).
    let matches: Vec<&str> = content
        .lines()
        .filter(|line| line.contains(pattern.as_str()))
        .collect();

    log::debug!("shell::builtin::grep: {} matching line(s)", matches.len());
    Ok(matches.join("\n"))
}

fn cmd_mount(args: &[String], vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.len() < 3 {
        return Err(String::from("mount: usage: mount <device> <path> <fstype>"));
    }
    log::info!(
        "shell::builtin::mount: {} on {} type {}",
        args[0],
        args[1],
        args[2]
    );
    vfs.mount(&args[0], &args[1], &args[2])?;
    Ok(String::new())
}

fn cmd_umount(args: &[String], vfs: &mut dyn Vfs) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("umount: missing operand"));
    }
    log::info!("shell::builtin::umount: {}", args[0]);
    vfs.umount(&args[0])?;
    Ok(String::new())
}

fn cmd_ps(sys: &dyn SystemInfo) -> Result<String, String> {
    log::info!("shell::builtin::ps: listing agent sessions");
    let agents = sys.list_agents();

    if agents.is_empty() {
        return Ok(String::from("No active agent sessions."));
    }

    let mut output = String::from("  ID  NAME                STATUS\n");
    for (id, name, status) in &agents {
        output.push_str(&format!("{:>4}  {:<20}{}\n", id, name, status));
    }
    Ok(output)
}

fn cmd_kill(args: &[String], sys: &mut dyn SystemInfo) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("kill: missing agent ID"));
    }
    let id: u64 = args[0]
        .parse()
        .map_err(|_| format!("kill: invalid ID: {}", args[0]))?;
    log::info!("shell::builtin::kill: killing agent {}", id);
    sys.kill_agent(id)?;
    Ok(format!("Agent {} terminated.", id))
}

fn cmd_clear(sys: &mut dyn SystemInfo) -> Result<String, String> {
    log::info!("shell::builtin::clear");
    sys.clear_screen();
    // Also emit ANSI clear as fallback
    Ok(String::from("\x1b[2J\x1b[H"))
}

fn cmd_help() -> Result<String, String> {
    log::info!("shell::builtin::help");
    Ok(String::from(
        "ClaudioOS Shell — Built-in Commands\n\
         ====================================\n\
         \n\
         File Operations:\n\
         \x20 ls [path]            List directory contents\n\
         \x20 cd [path]            Change working directory\n\
         \x20 pwd                  Print working directory\n\
         \x20 cat [file...]        Print file contents\n\
         \x20 echo [text...]       Print text\n\
         \x20 cp <src> <dst>       Copy file\n\
         \x20 mv <src> <dst>       Move/rename file\n\
         \x20 rm <path>            Remove file or directory\n\
         \x20 mkdir <path>         Create directory\n\
         \x20 touch <path>         Create empty file\n\
         \x20 head [-n N] [file]   Show first N lines (default 10)\n\
         \x20 tail [-n N] [file]   Show last N lines (default 10)\n\
         \x20 grep <pattern> [file] Search for pattern in file\n\
         \n\
         Filesystem:\n\
         \x20 mount <dev> <path> <type>  Mount filesystem\n\
         \x20 umount <path>              Unmount filesystem\n\
         \x20 df                         Show disk usage\n\
         \n\
         System:\n\
         \x20 ps                   List agent sessions\n\
         \x20 kill <id>            Kill agent session\n\
         \x20 clear                Clear screen\n\
         \x20 reboot               Reboot system\n\
         \x20 shutdown             Shutdown system\n\
         \x20 date                 Show current date/time\n\
         \x20 uptime               Show system uptime\n\
         \x20 free                 Show memory usage\n\
         \n\
         Network:\n\
         \x20 ifconfig             Show network interfaces\n\
         \x20 ping <host>          Ping a host\n\
         \x20 ssh <user@host>      SSH client (future)\n\
         \n\
         Environment:\n\
         \x20 set <key>=<value>    Set variable\n\
         \x20 unset <key>          Unset variable\n\
         \x20 export <key>=<value> Export variable\n\
         \n\
         Shell:\n\
         \x20 history              Show command history\n\
         \x20 help                 Show this help\n\
         \x20 exit                 Exit shell\n\
         \n\
         AI Mode:\n\
         \x20 Type natural language and the AI will interpret it.\n\
         \x20 Example: \"show me large files on the NVMe drive\"\n",
    ))
}

fn cmd_reboot(sys: &mut dyn SystemInfo) -> Option<Result<String, String>> {
    log::info!("shell::builtin::reboot: initiating system reboot");
    sys.reboot();
    // Never returns — but to satisfy the type system:
}

fn cmd_shutdown(sys: &mut dyn SystemInfo) -> Option<Result<String, String>> {
    log::info!("shell::builtin::shutdown: initiating system shutdown");
    sys.shutdown();
}

fn cmd_ifconfig(sys: &dyn SystemInfo) -> Result<String, String> {
    log::info!("shell::builtin::ifconfig");
    let ifaces = sys.ifconfig();

    if ifaces.is_empty() {
        return Ok(String::from("No network interfaces."));
    }

    let mut output = String::new();
    for (name, ip, mac, status) in &ifaces {
        output.push_str(&format!(
            "{}: {} ({})\n  HWaddr: {}\n\n",
            name, ip, status, mac
        ));
    }
    Ok(output)
}

fn cmd_ping(args: &[String], sys: &dyn SystemInfo) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("ping: missing host"));
    }
    log::info!("shell::builtin::ping: {}", args[0]);
    sys.ping(&args[0])
}

fn cmd_ssh(_args: &[String]) -> Result<String, String> {
    log::info!("shell::builtin::ssh: not yet implemented");
    Err(String::from("ssh: not yet implemented (see crates/sshd)"))
}

fn cmd_date(sys: &dyn SystemInfo) -> Result<String, String> {
    let d = sys.date();
    log::info!("shell::builtin::date: {}", d);
    Ok(d)
}

fn cmd_uptime(sys: &dyn SystemInfo) -> Result<String, String> {
    let secs = sys.uptime_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    let output = format!("up {} hours, {} minutes, {} seconds", hours, mins, s);
    log::info!("shell::builtin::uptime: {}", output);
    Ok(output)
}

fn cmd_free(sys: &dyn SystemInfo) -> Result<String, String> {
    let (total, used, free) = sys.memory_info();
    log::info!(
        "shell::builtin::free: total={}, used={}, free={}",
        total,
        used,
        free
    );

    let output = format!(
        "              total       used       free\n\
         Mem:    {:>10}  {:>10}  {:>10}\n",
        format_bytes(total),
        format_bytes(used),
        format_bytes(free),
    );
    Ok(output)
}

fn cmd_df(sys: &dyn SystemInfo) -> Result<String, String> {
    log::info!("shell::builtin::df");
    let disks = sys.disk_usage();

    if disks.is_empty() {
        return Ok(String::from("No mounted filesystems."));
    }

    let mut output = String::from("Filesystem       Size      Used     Avail  Mount\n");
    for (mount, total, used, avail) in &disks {
        output.push_str(&format!(
            "{:<16} {:>8}  {:>8}  {:>8}  {}\n",
            mount,
            format_bytes(*total),
            format_bytes(*used),
            format_bytes(*avail),
            mount,
        ));
    }
    Ok(output)
}

fn cmd_set(args: &[String], env: &mut Environment) -> Result<String, String> {
    if args.is_empty() {
        // Show all variables
        let vars = env.all();
        let output: Vec<String> = vars.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        return Ok(output.join("\n"));
    }
    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let key = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            log::info!("shell::builtin::set: {}={}", key, value);
            env.set(key, value);
        } else {
            return Err(format!("set: invalid format: {} (expected KEY=VALUE)", arg));
        }
    }
    Ok(String::new())
}

fn cmd_unset(args: &[String], env: &mut Environment) -> Result<String, String> {
    if args.is_empty() {
        return Err(String::from("unset: missing variable name"));
    }
    for arg in args {
        log::info!("shell::builtin::unset: {}", arg);
        env.unset(arg);
    }
    Ok(String::new())
}

fn cmd_export(args: &[String], env: &mut Environment) -> Result<String, String> {
    if args.is_empty() {
        let vars = env.exported_vars();
        let output: Vec<String> = vars
            .iter()
            .map(|(k, v)| format!("export {}={}", k, v))
            .collect();
        return Ok(output.join("\n"));
    }
    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let key = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            log::info!("shell::builtin::export: {}={}", key, value);
            env.set_export(key, value);
        } else {
            // export VAR (mark existing as exported)
            log::info!("shell::builtin::export: {}", arg);
            env.export(arg);
        }
    }
    Ok(String::new())
}

/// Format a byte count as a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}
