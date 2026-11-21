#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalCalendar {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
}

pub(crate) fn from_unix_seconds(seconds: i64) -> Option<LocalCalendar> {
    let (year, month, day, hour, min, sec) = mio_core::codec_util::local_datetime_parts(seconds)?;
    Some(LocalCalendar {
        year,
        month: month as i32,
        day: day as i32,
        hour: hour as i32,
        min: min as i32,
        sec: sec as i32,
    })
}
