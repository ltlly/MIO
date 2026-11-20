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
    #[cfg(unix)]
    {
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        let local = unsafe {
            if libc::localtime_r(&seconds, local.as_mut_ptr()).is_null() {
                return None;
            }
            local.assume_init()
        };
        let year = local.tm_year.checked_add(1900)?;
        Some(LocalCalendar {
            year,
            month: local.tm_mon + 1,
            day: local.tm_mday,
            hour: local.tm_hour,
            min: local.tm_min,
            sec: local.tm_sec,
        })
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn FileTimeToSystemTime(ft: *const FileTime, st: *mut SystemTime) -> i32;
            fn SystemTimeToTzSpecificLocalTime(
                tz: *const std::ffi::c_void,
                utc: *const SystemTime,
                local: *mut SystemTime,
            ) -> i32;
        }
        const EPOCH_DIFF_100NS: i128 = 116_444_736_000_000_000;
        let ticks = EPOCH_DIFF_100NS.checked_add((seconds as i128).checked_mul(10_000_000)?)?;
        if ticks < 0 || ticks > u64::MAX as i128 {
            return None;
        }
        let ticks_u64 = ticks as u64;
        let ft = FileTime {
            low: ticks_u64 as u32,
            high: (ticks_u64 >> 32) as u32,
        };
        let mut utc = SystemTime::default();
        let mut local = SystemTime::default();
        unsafe {
            if FileTimeToSystemTime(&ft, &mut utc) == 0 {
                return None;
            }
            if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
                return None;
            }
        }
        Some(LocalCalendar {
            year: i32::from(local.year),
            month: i32::from(local.month),
            day: i32::from(local.day),
            hour: i32::from(local.hour),
            min: i32::from(local.minute),
            sec: i32::from(local.second),
        })
    }
}
