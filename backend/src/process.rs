//! Process management utilities for killing processes and their children.

use std::collections::HashSet;
use tracing::{debug, info, trace, warn};

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    pub fn kill_process_tree(root_pid: i32) {
        debug!(
            pid = root_pid,
            "Starting process tree termination (Windows)"
        );
        let mut pids_to_kill = HashSet::new();
        collect_children(root_pid, &mut pids_to_kill);
        if pids_to_kill.is_empty() {
            warn!(pid = root_pid, "No processes found to kill");
            return;
        }
        info!(
            count = pids_to_kill.len(),
            "Terminating processes with TerminateProcess"
        );
        for &pid in &pids_to_kill {
            trace!(pid = pid, "Terminating process");
            unsafe {
                match OpenProcess(PROCESS_TERMINATE, false, pid as u32) {
                    Ok(handle) => {
                        if !handle.is_invalid() {
                            let _ = TerminateProcess(handle, 1);
                            let _ = CloseHandle(handle);
                        }
                    }
                    Err(_) => trace!(pid = pid, "Failed to open process"),
                }
            }
        }
        debug!("Waiting for process termination (1000ms)");
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    fn collect_children(parent_pid: i32, pids: &mut HashSet<i32>) {
        pids.insert(parent_pid);
        unsafe {
            match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                Ok(snapshot) => {
                    if snapshot.is_invalid() {
                        return;
                    }
                    let result = (|| {
                        let mut pids_ref = pids;
                        let mut entry = PROCESSENTRY32W {
                            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                            ..Default::default()
                        };
                        if Process32FirstW(snapshot, &mut entry).as_bool() {
                            loop {
                                let pid = entry.th32ProcessID as i32;
                                let ppid = entry.th32ParentProcessID as i32;
                                if ppid == parent_pid && !pids_ref.contains(&pid) {
                                    trace!(
                                        parent_pid = parent_pid,
                                        child_pid = pid,
                                        "Found child process (Windows)"
                                    );
                                    collect_children(pid, pids_ref);
                                }
                                if !Process32NextW(snapshot, &mut entry).as_bool() {
                                    break;
                                }
                            }
                        }
                        Ok(())
                    })();
                    let _ = CloseHandle(snapshot);
                }
                Err(_) => trace!("Failed to create toolhelp snapshot"),
            }
        }
    }
}

#[cfg(not(target_os = "windows"))] // Assumes libc-based Unix-like system (Linux, macOS, Android, iOS)
mod platform {
    use super::*;
    /// Recursively kill a process and all its children using /proc filesystem.
    /// This approach works even with proot since we're killing the actual parent process.
    pub fn kill_process_tree(root_pid: i32) {
        debug!(pid = root_pid, "Starting process tree termination");

        // First pass: collect all process IDs to kill
        let mut pids_to_kill = HashSet::new();
        collect_children(root_pid, &mut pids_to_kill);

        if pids_to_kill.is_empty() {
            warn!(pid = root_pid, "No processes found to kill");
            return;
        }

        info!(
            count = pids_to_kill.len(),
            "Terminating processes with SIGTERM"
        );
        // Kill with SIGTERM first
        for &pid in &pids_to_kill {
            trace!(pid = pid, "Sending SIGTERM");
            unsafe {
                let _ = libc::kill(pid, libc::SIGTERM);
            }
        }

        // Wait for graceful shutdown
        debug!("Waiting for graceful shutdown (1000ms)");
        std::thread::sleep(std::time::Duration::from_millis(1000));

        // Check which processes are still alive
        let mut still_alive = HashSet::new();
        for &pid in &pids_to_kill {
            if is_process_alive(pid) {
                still_alive.insert(pid);
            }
        }

        // Force kill with SIGKILL if any are still alive
        if !still_alive.is_empty() {
            info!(
                count = still_alive.len(),
                "Processes still running, force killing with SIGKILL"
            );
            for &pid in &still_alive {
                trace!(pid = pid, "Sending SIGKILL");
                unsafe {
                    let _ = libc::kill(pid, libc::SIGKILL);
                }
            }

            // Final wait
            debug!("Final wait for process termination (500ms)");
            std::thread::sleep(std::time::Duration::from_millis(500));
        } else {
            info!("All processes terminated gracefully");
        }
    }

    /// Recursively collect all child process IDs
    fn collect_children(parent_pid: i32, pids: &mut HashSet<i32>) {
        // Add the parent itself
        pids.insert(parent_pid);
        debug!(parent_pid = parent_pid, "Collecting child processes");

        // Try to read /proc/[pid]/task/[pid]/children to get direct children
        let children_path = format!("/proc/{}/task/{}/children", parent_pid, parent_pid);
        if let Ok(children_str) = std::fs::read_to_string(&children_path) {
            debug!(parent_pid = parent_pid, children_file = %children_path, "Found children file");
            for child_str in children_str.split_whitespace() {
                if let Ok(child_pid) = child_str.parse::<i32>() {
                    if !pids.contains(&child_pid) {
                        trace!(
                            parent_pid = parent_pid,
                            child_pid = child_pid,
                            "Found child process"
                        );
                        collect_children(child_pid, pids);
                    }
                }
            }
        } else {
            // Fallback: try searching /proc directly for children (slower but more compatible)
            trace!(
                parent_pid = parent_pid,
                "Children file not found, using fallback /proc scan"
            );
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(pid_str) = path.file_name().and_then(|n| n.to_str()) {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            if let Ok(ppid) = read_ppid(pid) {
                                if ppid == parent_pid && !pids.contains(&pid) {
                                    trace!(
                                        parent_pid = parent_pid,
                                        child_pid = pid,
                                        "Found child process via fallback"
                                    );
                                    collect_children(pid, pids);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check if a process is still alive
    fn is_process_alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Read the parent PID of a process
    fn read_ppid(pid: i32) -> anyhow::Result<i32> {
        let status_path = format!("/proc/{}/stat", pid);
        let content = std::fs::read_to_string(status_path)?;
        // The format is: pid (comm) state ppid ...
        // Find the last closing paren to handle comm with spaces/parens
        if let Some(last_paren) = content.rfind(')') {
            let rest = &content[last_paren + 1..];
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() > 1 {
                return parts[1].parse::<i32>().map_err(|e| anyhow::anyhow!(e));
            }
        }
        Err(anyhow::anyhow!(
            "Could not parse ppid from /proc/{}/stat",
            pid
        ))
    }
}

pub use platform::kill_process_tree;
