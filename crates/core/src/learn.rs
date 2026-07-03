//! Turning measured run history into planning estimates.
//!
//! We can directly measure how long a job actually ran (finish − start), so we
//! learn a job's runtime from its own history instead of trusting a one-off
//! guess. The estimate is biased slightly long on purpose: under-estimating a
//! deadline-bound job's duration risks starting too late and missing the
//! deadline, which is worse than reserving a little extra cheap time.
//!
//! Power is deliberately *not* learned here: spotwatt has no power meter, so
//! `power_watts` stays a user estimate. Wire up a smart plug or node power
//! sensor later and the same instance-from-history pattern applies.

use std::collections::HashMap;

/// Extra headroom added to the mean so consistent jobs still get a small buffer.
const SAFETY_MARGIN: f64 = 1.15;

/// Runs of the exact command needed before an estimate is trusted.
pub const MIN_SAMPLES: usize = 3;

/// Newest measured runs kept per command / signature.
const MAX_SAMPLES: usize = 10;

/// Estimate planning minutes from recent measured run durations (in minutes).
///
/// Returns `None` until at least `min_samples` positive samples exist. The
/// estimate is the larger of (mean × safety margin) and the longest observed
/// run, so it never under-covers a duration we've actually seen.
pub fn estimate_minutes(samples: &[i64], min_samples: usize) -> Option<i64> {
    if samples.len() < min_samples.max(1) {
        return None;
    }
    // Every sample is a real measured run (the caller only passes completed
    // runs). Clamp clock-skew negatives to zero; a very fast run is a legitimate
    // 0-minute sample and must still count toward the history.
    let clean: Vec<i64> = samples.iter().map(|&m| m.max(0)).collect();
    let mean = clean.iter().sum::<i64>() as f64 / clean.len() as f64;
    let padded = (mean * SAFETY_MARGIN).ceil() as i64;
    let longest = *clean.iter().max().unwrap();
    // Floor at 1: the scheduler plans in whole minutes / hourly slots, so even a
    // sub-minute command occupies at least one minute.
    Some(padded.max(longest).max(1))
}

/// A learned-duration estimate and how it was derived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub minutes: i64,
    pub runs: usize,
    /// True when based on runs of this exact command; false when based on
    /// runs of *similar* commands (same signature).
    pub exact: bool,
}

/// Reduce a shell command to the part that predicts its runtime: the program
/// and its subcommand words — not argument values. `echo 212` and `echo 1`
/// are the same command for time/cost purposes; so are two `ffmpeg -i <file>`
/// transcodes, roughly. Environment assignments and common wrappers (`sudo`,
/// `nice`, `timeout`, …) are skipped so they don't hide the real program.
///
/// Compound commands are the exception: anything with unquoted shell control
/// operators (`;`, `&`, `|`, command substitution) keeps the whole line as its
/// identity. Reducing `echo ok && train.sh` to `echo` would let an hour-long
/// script borrow the runtime history of `echo`.
///
/// Tokenization uses shlex (quotes and escapes handled like a POSIX shell);
/// if the command doesn't tokenize (unbalanced quotes), it falls back to
/// whitespace splitting.
pub fn command_signature(command: &str) -> String {
    const WRAPPERS: &[&str] = &[
        "sudo", "doas", "nice", "ionice", "nohup", "time", "timeout", "env", "stdbuf",
    ];
    const INTERPRETERS: &[&str] = &[
        "sh", "bash", "zsh", "fish", "python", "python3", "python2", "py", "pypy", "node",
        "ruby", "perl", "deno", "bun",
    ];

    if has_shell_operators(command) {
        return command.trim().to_string();
    }

    let tokens = shlex::split(command)
        .unwrap_or_else(|| command.split_whitespace().map(str::to_string).collect());

    let mut i = 0;
    // Find the real program: skip env assignments, wrappers, and the flags
    // and numeric arguments that belong to the wrappers (`nice -n 10 …`,
    // `timeout 300 …`).
    let mut program: Option<String> = None;
    while i < tokens.len() {
        let t = &tokens[i];
        i += 1;
        if t.starts_with('-') || is_numberish(t) || t.contains('=') {
            continue;
        }
        let base = basename(t);
        if WRAPPERS.contains(&base) {
            continue;
        }
        program = Some(base.to_string());
        break;
    }
    let Some(program) = program else {
        return command.trim().to_string();
    };
    let mut sig = vec![program.clone()];

    if INTERPRETERS.contains(&program.as_str()) {
        // For interpreters the script is the real identity: `python3 train.py`
        // and `python3 quick.py` must not pool. Interpreter flags are skipped;
        // an inline script (`sh -c '…'`) becomes the identity itself.
        for t in &tokens[i..] {
            if t.starts_with('-') {
                continue;
            }
            sig.push(basename(t).to_string());
            break;
        }
        return sig.join(" ");
    }

    // Collect subcommand-like words (`restic backup`, `docker compose up`),
    // stopping at the first flag, path, or value — from there on it's data.
    // One exception: a script-file argument is identity, not data — `py
    // 1_min_task.py` and `py 1_hrs_task.py` must not pool, even when the
    // program isn't a recognized interpreter.
    for t in &tokens[i..] {
        if is_script_file(t) {
            sig.push(basename(t).to_string());
            break;
        }
        if sig.len() >= 3 || !is_wordish(t) {
            break;
        }
        sig.push(t.clone());
    }
    sig.join(" ")
}

/// True when `command` contains shell control operators outside quotes:
/// `;`, `&`, `|`, backticks, `$(`, or a newline. Such a line is really a
/// compound script — several programs in a trenchcoat — and must not be
/// reduced to its first word for learning purposes. `$(…)` and backticks
/// count even inside double quotes, where the shell still runs them.
pub fn has_shell_operators(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut prev = '\0';
    for c in command.chars() {
        if escaped {
            escaped = false;
            prev = c;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '`' if !in_single => return true,
            '(' if prev == '$' && !in_single => return true,
            ';' | '&' | '|' | '\n' if !in_single && !in_double => return true,
            _ => {}
        }
        prev = c;
    }
    false
}

/// A token whose basename ends in a script extension — the kind of argument
/// that *is* the program rather than data fed to it.
fn is_script_file(s: &str) -> bool {
    const EXTS: &[&str] = &[
        ".py", ".sh", ".bash", ".zsh", ".js", ".mjs", ".cjs", ".ts", ".rb", ".pl", ".php",
        ".lua",
    ];
    let b = basename(s).to_ascii_lowercase();
    EXTS.iter().any(|e| b.len() > e.len() && b.ends_with(e))
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn is_numberish(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ':' | ','))
}

fn is_wordish(s: &str) -> bool {
    !s.starts_with('-')
        && !is_numberish(s)
        && s.chars().any(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Run history indexed for estimation: by exact command, and by command
/// signature as a fallback when the exact command hasn't run often enough.
pub struct DurationLearner {
    exact: HashMap<String, Vec<i64>>,
    by_sig: HashMap<String, Vec<i64>>,
}

impl DurationLearner {
    /// Build from completed runs as `(command, measured minutes)`, most recent
    /// first. Only the newest runs per command/signature are kept.
    pub fn from_history(runs: impl IntoIterator<Item = (String, i64)>) -> Self {
        let mut exact: HashMap<String, Vec<i64>> = HashMap::new();
        let mut by_sig: HashMap<String, Vec<i64>> = HashMap::new();
        for (command, minutes) in runs {
            let sig = command_signature(&command);
            let s = by_sig.entry(sig).or_default();
            if s.len() < MAX_SAMPLES {
                s.push(minutes);
            }
            let e = exact.entry(command).or_default();
            if e.len() < MAX_SAMPLES {
                e.push(minutes);
            }
        }
        DurationLearner { exact, by_sig }
    }

    /// Estimate for `command`: its own history when there's enough of it,
    /// otherwise the pooled history of same-signature commands.
    pub fn estimate(&self, command: &str) -> Option<Estimate> {
        if let Some(samples) = self.exact.get(command) {
            if let Some(minutes) = estimate_minutes(samples, MIN_SAMPLES) {
                return Some(Estimate { minutes, runs: samples.len(), exact: true });
            }
        }
        let samples = self.by_sig.get(&command_signature(command))?;
        let minutes = estimate_minutes(samples, MIN_SAMPLES)?;
        Some(Estimate { minutes, runs: samples.len(), exact: false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_until_enough_samples() {
        assert_eq!(estimate_minutes(&[40, 41], 3), None);
        assert!(estimate_minutes(&[40, 41, 42], 3).is_some());
    }

    #[test]
    fn counts_fast_sub_minute_runs() {
        // Three quick commands (each rounds to 0 min) still produce a usable,
        // floored estimate — they must not be silently dropped.
        assert_eq!(estimate_minutes(&[0, 0, 0], 3), Some(1));
    }

    #[test]
    fn consistent_runs_get_margin() {
        // mean 40 × 1.15 = 46, which exceeds the longest run (40).
        assert_eq!(estimate_minutes(&[40, 40, 40], 3), Some(46));
    }

    #[test]
    fn never_under_covers_the_longest_run() {
        // mean 50 × 1.15 = 57.5 → 58, but one run took 90, so estimate is 90.
        assert_eq!(estimate_minutes(&[30, 30, 90], 3), Some(90));
    }

    // -- command signatures --

    #[test]
    fn signature_drops_argument_values() {
        assert_eq!(command_signature("echo 212"), "echo");
        assert_eq!(command_signature("echo 1"), "echo");
        assert_eq!(command_signature("ffmpeg -i ep01.mkv out.mkv"), "ffmpeg");
    }

    #[test]
    fn signature_keeps_subcommands() {
        assert_eq!(command_signature("restic backup /data"), "restic backup");
        assert_eq!(command_signature("git gc --aggressive"), "git gc");
        assert_eq!(command_signature("docker compose up -d"), "docker compose up");
    }

    #[test]
    fn signature_skips_wrappers_and_env() {
        assert_eq!(command_signature("sudo restic backup /data"), "restic backup");
        assert_eq!(command_signature("nice -n 10 restic backup /x"), "restic backup");
        assert_eq!(command_signature("timeout 300 rsync -a /a /b"), "rsync");
        assert_eq!(command_signature("RUST_LOG=debug cargo build"), "cargo build");
    }

    #[test]
    fn signature_keeps_interpreter_scripts_apart() {
        assert_eq!(command_signature("python3 train.py --epochs 5"), "python3 train.py");
        assert_eq!(command_signature("python3 quick.py"), "python3 quick.py");
        assert_eq!(command_signature("bash /opt/scripts/backup.sh"), "bash backup.sh");
    }

    #[test]
    fn signature_strips_program_path() {
        assert_eq!(command_signature("/usr/local/bin/restic backup"), "restic backup");
    }

    #[test]
    fn signature_survives_unbalanced_quotes() {
        // shlex fails on this; the whitespace fallback still yields something.
        assert_eq!(command_signature("echo 'unterminated"), "echo");
    }

    #[test]
    fn compound_commands_keep_their_whole_line() {
        // `echo …` chained to a long script must not borrow echo's history.
        for cmd in [
            "echo 3222 & py biggest_script_in_world.py",
            "echo ok && ./train.sh",
            "sleep 1; restic backup /data",
            "cat list | xargs -n1 transcode",
            "echo `date`",
            "echo $(heavy-thing)",
            "echo \"$(heavy-thing)\"", // $() runs even inside double quotes
        ] {
            assert_eq!(command_signature(cmd), cmd, "must stay exact: {cmd}");
        }
    }

    #[test]
    fn quoted_operators_are_just_data() {
        assert!(!has_shell_operators("echo 'a;b & c|d'"));
        assert!(!has_shell_operators("echo \"a;b\""));
        assert!(!has_shell_operators("echo a\\;b"));
        assert_eq!(command_signature("echo 'a;b'"), "echo");
    }

    #[test]
    fn script_arguments_are_identity_even_for_unknown_programs() {
        // `py` (the Python launcher) and arbitrary runners: the script decides
        // the runtime, so different scripts must not pool.
        assert_eq!(command_signature("py 1_min_task.py"), "py 1_min_task.py");
        assert_eq!(command_signature("py 1_hrs_task.py"), "py 1_hrs_task.py");
        assert_ne!(
            command_signature("py 1_min_task.py"),
            command_signature("py 1_hrs_task.py")
        );
        assert_eq!(command_signature("myrunner task.sh --fast"), "myrunner task.sh");
    }

    // -- learner: exact history preferred, signature as fallback --

    #[test]
    fn learner_prefers_exact_history() {
        let learner = DurationLearner::from_history(vec![
            ("restic backup /data".to_string(), 40),
            ("restic backup /data".to_string(), 42),
            ("restic backup /data".to_string(), 44),
            ("restic backup /other".to_string(), 400),
            ("restic backup /other".to_string(), 400),
            ("restic backup /other".to_string(), 400),
        ]);
        let est = learner.estimate("restic backup /data").unwrap();
        assert!(est.exact);
        assert_eq!(est.runs, 3);
        assert!(est.minutes < 100, "must not be polluted by /other: {est:?}");
    }

    #[test]
    fn learner_falls_back_to_similar_commands() {
        let learner = DurationLearner::from_history(vec![
            ("ffmpeg -i ep01.mkv a.mkv".to_string(), 30),
            ("ffmpeg -i ep02.mkv b.mkv".to_string(), 34),
            ("ffmpeg -i ep03.mkv c.mkv".to_string(), 32),
        ]);
        // Never ran this exact file, but three similar transcodes exist.
        let est = learner.estimate("ffmpeg -i ep04.mkv d.mkv").unwrap();
        assert!(!est.exact);
        assert_eq!(est.runs, 3);
    }

    #[test]
    fn learner_none_when_nothing_matches() {
        let learner = DurationLearner::from_history(vec![
            ("restic backup /data".to_string(), 40),
        ]);
        assert!(learner.estimate("ffmpeg -i x.mkv y.mkv").is_none());
    }
}
