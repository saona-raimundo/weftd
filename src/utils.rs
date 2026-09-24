use time::OffsetDateTime;
use time::macros::format_description;

/// Local timestamp for filenames: 2026-09-23_18-43-07
pub fn file_timestamp() -> String {
    let fmt = format_description!(version = 2, "[year]-[month]-[day]_[hour]-[minute]-[second]");
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(fmt)
        .expect("static format description is valid")
}
