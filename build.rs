use std::process::Command;

fn main() {
    // Rebuild when the commit changes.
    println!("cargo:rerun-if-changed=.git/HEAD");

    // --- Git hash ---
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={git_hash}");

    // --- Build date ---
    // Honour SOURCE_DATE_EPOCH for reproducible builds.
    let build_date = if let Ok(epoch_str) = std::env::var("SOURCE_DATE_EPOCH") {
        epoch_to_date(epoch_str.trim().parse::<u64>().unwrap_or(0))
    } else {
        Command::new("date")
            .arg("+%Y-%m-%d")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };

    println!("cargo:rustc-env=BUILD_DATE={build_date}");
}

/// Convert a Unix timestamp (seconds since epoch) to a `YYYY-MM-DD` string
/// using only `std` — no external crates required.
fn epoch_to_date(secs: u64) -> String {
    // Days since Unix epoch (1970-01-01).
    let days = (secs / 86400) as u32;

    // Gregorian calendar arithmetic.
    let mut year = 1970u32;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let month_days: [u32; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for &md in &month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        month += 1;
    }

    let day = remaining + 1;
    format!("{year:04}-{month:02}-{day:02}")
}

fn is_leap(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
