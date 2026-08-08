// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: doctor.rs
//  script_path: execviz-rs/src/doctor.rs
//  module_name: doctor
//  version: 0.53.1
//  description: Can this machine run it? Asked before anything is installed.
//  kind: module
//  spec: internal
//  internal_dependencies: json
//  external_dependencies: 
//  features: doctor
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! Can this machine run it? Asked before anything is installed.
//!
//! An install that succeeds and then does not work differs from one that
//! refuses, because the operator now has a mystery instead of a message. Every
//! check here names the requirement, what was found, and the fix.
//!
//! Nothing is fetched and nothing is installed. This only looks.

use crate::json::J;

// ========================================================================
// TYPES
// ========================================================================

struct Check {
    what: &'static str,
    ok: bool,
    found: String,
    fix: String,
}

// ========================================================================
// INTERNALS
// ========================================================================

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn uname_machine() -> String {
    // /proc is the portable-enough source here, and this is Linux-only anyway
    std::process::Command::new("uname").arg("-m").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn kernel_version() -> (u32, u32, String) {
    let rel = read("/proc/sys/kernel/osrelease").unwrap_or_default();
    let mut it = rel.split(|c: char| !c.is_ascii_digit());
    let maj = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (maj, min, rel)
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn diagnose() -> (J, bool) {
    let mut checks: Vec<Check> = Vec::new();

// ========================================================================
// THE FLOOR'S FOUR HARD REQUIREMENTS
// ========================================================================
    let machine = uname_machine();
    checks.push(Check {
        what: "architecture",
        ok: machine == "x86_64",
        found: machine.clone(),
        fix: "the recorder is x86_64 only: its register offsets and syscall numbers are \
              architecture-specific, and aarch64 needs a second table rather than a \
              recompile. The collector, the map and the adapters run here regardless.".into(),
    });

    let (maj, min, rel) = kernel_version();
    let kernel_ok = maj > 5 || (maj == 5 && min >= 8);
    checks.push(Check {
        what: "kernel",
        ok: kernel_ok,
        found: if rel.is_empty() { "unknown".into() } else { rel },
        fix: "BPF ring buffers need 5.8 or newer and reading user memory from a probe \
              needs 5.5. RHEL 8, Ubuntu 18.04 and Debian 10 are below that line; RHEL 9, \
              Ubuntu 22.04 and Debian 12 are above it.".into(),
    });

    // Capabilities: root passes, otherwise look at what this process holds.
    let euid_root = read("/proc/self/status")
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.to_string()))
        .map(|l| l.split_whitespace().nth(2).map(|u| u == "0").unwrap_or(false))
        .unwrap_or(false);
    let capeff = read("/proc/self/status")
        .and_then(|s| s.lines().find(|l| l.starts_with("CapEff:")).map(|l| l.to_string()))
        .and_then(|l| l.split_whitespace().nth(1).map(|h| h.to_string()))
        .unwrap_or_default();
    // CAP_BPF is bit 39, CAP_PERFMON bit 38, CAP_SYS_ADMIN bit 21
    let caps = u64::from_str_radix(&capeff, 16).unwrap_or(0);
    let has_bpf = caps & (1 << 39) != 0;
    let has_perfmon = caps & (1 << 38) != 0;
    let has_sysadmin = caps & (1 << 21) != 0;
    let cap_ok = euid_root || (has_bpf && has_perfmon) || has_sysadmin;
    checks.push(Check {
        what: "privilege",
        ok: cap_ok,
        found: if euid_root { "running as root".into() }
               else { format!("CapEff {} (bpf={} perfmon={} sys_admin={})",
                              if capeff.is_empty() { "unknown" } else { &capeff },
                              has_bpf, has_perfmon, has_sysadmin) },
        fix: "grant the two capabilities the recorder needs, rather than running it as root:\n    \
              setcap cap_bpf,cap_perfmon+ep /usr/local/bin/execviz-record\n  \
              A container needs them in its own capability set, not only on the host. Most \
              shared hosting and managed platforms cannot grant them at all.".into(),
    });

    let lockdown = read("/sys/kernel/security/lockdown").unwrap_or_default();
    let locked = lockdown.contains("[integrity]") || lockdown.contains("[confidentiality]");
    checks.push(Check {
        what: "kernel lockdown",
        ok: !locked,
        found: if lockdown.is_empty() { "not enabled".into() } else { lockdown },
        fix: "lockdown blocks loading BPF programs and is commonly switched on by secure \
              boot. Disabling secure boot, or signing a kernel that permits it, is the only \
              way past this; nothing in this tool can work around it.".into(),
    });

// ========================================================================
// WHAT STILL WORKS WHEN THE FLOOR CANNOT RUN
// ========================================================================
    let recorder_ok = checks.iter().all(|c| c.ok);

    let rows: Vec<J> = checks.iter().map(|c| J::Obj([
        ("check".to_string(), J::Str(c.what.to_string())),
        ("ok".to_string(), J::Bool(c.ok)),
        ("found".to_string(), J::Str(c.found.clone())),
        ("fix".to_string(), if c.ok { J::Null } else { J::Str(c.fix.clone()) }),
    ].into_iter().collect())).collect();

    let out = J::Obj([
        ("recorder".to_string(), J::Str(
            if recorder_ok { "this machine can run the recorder".to_string() }
            else { "this machine cannot run the recorder; the reasons are below".to_string() })),
        ("collector_and_map".to_string(), J::Str(
            "run here regardless: they need no privileges, no kernel features, and no \
             particular architecture".to_string())),
        ("adapters".to_string(), J::Str(
            "run here regardless: each is a source file using its own language's standard \
             library".to_string())),
        ("checks".to_string(), J::Arr(rows)),
        ("note".to_string(), J::Str(
            "Nothing was installed or fetched by this command. It only looked.".to_string())),
    ].into_iter().collect());
    (out, recorder_ok)
}

/// A report an operator can paste into an issue.
///
/// This is the compatibility matrix filled in by the people who have the
/// machines, which for a tool given away is the only way it ever gets filled in
/// accurately. A distribution nobody has run it on is listed as untested rather
/// than as supported.
///
/// It carries what the checks found and nothing else. No hostname, no user, no
/// paths, no process names: a compatibility report should be safe to paste in
/// public without reading it first.
pub fn report() -> J {
    let (diag, ok) = diagnose();

    // Distribution name and version, which is what a reader recognises even
    // though the kernel is what decides.
    let mut distro = "unknown".to_string();
    if let Ok(s) = std::fs::read_to_string("/etc/os-release") {
        let mut name = String::new();
        let mut ver = String::new();
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") { name = v.trim_matches('"').to_string(); }
            if let Some(v) = line.strip_prefix("VERSION_ID=") { ver = v.trim_matches('"').to_string(); }
        }
        if !name.is_empty() { distro = name } else if !ver.is_empty() { distro = ver }
    }

    // Whether the binary is statically linked, because a dynamic one is the
    // usual reason a release works on the machine that built it and nowhere else.
    let linkage = if std::fs::read_to_string("/proc/self/maps")
        .map(|m| m.contains("libc.so")).unwrap_or(false) {
        "dynamic (this binary depends on the host's libc version)"
    } else {
        "static (no libc dependency)"
    };

    J::Obj([
        ("distribution".to_string(), J::Str(distro)),
        ("linkage".to_string(), J::Str(linkage.to_string())),
        ("floor_supported".to_string(), J::Bool(ok)),
        ("diagnosis".to_string(), diag),
        ("paste_this".to_string(), J::Str(
            "This report carries no hostname, user, path or process name. It is              safe to paste in public, and doing so is how the compatibility table              gets filled in by people who have the machines.".to_string())),
    ].into_iter().collect())
}
