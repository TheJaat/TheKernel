//! The date: what a real-time clock says, turned into the one number
//! the rest of the system uses - seconds since 1 January 1970, UTC
//! ("Unix time").
//!
//! A real-time clock is a small battery-powered counter that keeps
//! going while the machine is off. TheTaaJ reads it once, at boot, and
//! from then on counts with the CPU's own counter (clock.rs): the
//! real-time clock is slow to read and only says whole seconds.
//!
//! The two machines' clocks differ in every way that matters here:
//!
//! - The PC's (the "CMOS clock", Motorola's MC146818 from 1984) keeps
//!   the date as a calendar - second, minute, hour, day, month, year -
//!   in registers reached through I/O ports 0x70 and 0x71, often in
//!   BCD (each decimal digit in four bits: 0x59 is 59), sometimes with
//!   a 12-hour clock, and with a two-digit year. Turning that into
//!   seconds is this module's work.
//! - AArch64's PL031 (an ARM PrimeCell) is one 32-bit register that
//!   counts seconds - already Unix time, nothing to convert.
//!
//! Everything here is plain arithmetic, so it is tested on the host.

/// A moment as a calendar says it, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: u32,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Days from 1 January 1970 to the given day of the (proleptic)
/// Gregorian calendar - negative before it.
///
/// Howard Hinnant's method: count from 1 March of year 0, so that the
/// awkward month, February, comes last in each year and its extra day
/// changes no other month's offset. A 400-year "era" is exactly
/// 146097 days, which takes care of the leap-year rules (every fourth
/// year, not every hundredth, but every four hundredth).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400; // 0..=399
    let month = i64::from(month);
    // March is month 0 of the shifted year.
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    // 719468: days from 1 March of year 0 to 1 January 1970.
    era * 146097 + day_of_era - 719468
}

/// The number of days in a month.
pub fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

impl Date {
    /// Seconds since 1970 - or None for a date that is not one (a 31st
    /// of April, an hour 24, a year before 1970): what a clock whose
    /// battery has died, or that was never set, may say.
    pub fn unix_seconds(&self) -> Option<u64> {
        let valid = self.year >= 1970
            && (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60;
        if !valid {
            return None;
        }
        let days = days_from_civil(i64::from(self.year), self.month, self.day) as u64;
        Some(days * 86400 + u64::from(self.hour) * 3600 + u64::from(self.minute) * 60 + u64::from(self.second))
    }
}

impl Date {
    /// The date `seconds` after 1970 began: `days_from_civil` run
    /// backwards (Hinnant's `civil_from_days`).
    pub fn from_unix_seconds(seconds: u64) -> Date {
        let days = (seconds / 86400) as i64 + 719468;
        let era = days.div_euclid(146097);
        let day_of_era = days - era * 146097;
        let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153; // March is 0
        let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
        let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
        let year = (year_of_era + era * 400 + i64::from(month <= 2)) as u32;
        let time = seconds % 86400;
        Date { year, month, day, hour: (time / 3600) as u32, minute: (time / 60 % 60) as u32, second: (time % 60) as u32 }
    }
}

impl core::fmt::Display for Date {
    /// ISO 8601: 2026-11-25 13:45:30.
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

/// A BCD byte's value: 0x59 is 59.
pub fn from_bcd(byte: u8) -> u8 {
    (byte >> 4) * 10 + (byte & 0x0F)
}

/// The CMOS clock's registers, as read (cmos.rs in the x86 code reads
/// them). Their numbers in the chip: second 0x00, minute 0x02, hour
/// 0x04, day 0x07, month 0x08, year 0x09, status B 0x0B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmosRegisters {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u8,
    /// Status register B: bit 1 set means a 24-hour clock, bit 2 set
    /// means binary rather than BCD.
    pub status_b: u8,
}

/// Status B's bits.
const HOURS_24: u8 = 1 << 1;
const BINARY: u8 = 1 << 2;
/// In a 12-hour clock, the hour's top bit says afternoon.
const PM: u8 = 0x80;

impl CmosRegisters {
    /// The date the registers say.
    ///
    /// The year is two digits. Some PCs keep the century in another
    /// register, but where it is depends on the machine (ACPI's FADT
    /// names it, if anywhere); TheTaaJ takes every year to be in this
    /// century, and so will be wrong in 2100.
    pub fn date(&self) -> Date {
        let value = |byte: u8| if self.status_b & BINARY != 0 { byte } else { from_bcd(byte) };
        let mut hour = value(self.hour & !PM);
        if self.status_b & HOURS_24 == 0 {
            // 12 AM is midnight (0), 12 PM noon (12); 1 PM is 13.
            hour %= 12;
            if self.hour & PM != 0 {
                hour += 12;
            }
        }
        Date {
            year: 2000 + u32::from(value(self.year)),
            month: u32::from(value(self.month)),
            day: u32::from(value(self.day)),
            hour: u32::from(hour),
            minute: u32::from(value(self.minute)),
            second: u32::from(value(self.second)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: u32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Date {
        Date { year, month, day, hour, minute, second }
    }

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(date(1970, 1, 1, 0, 0, 0).unix_seconds(), Some(0));
    }

    // Checked against Python's calendar.timegm.
    #[test]
    fn known_dates() {
        assert_eq!(date(2026, 11, 25, 13, 45, 30).unix_seconds(), Some(1_795_614_330));
        assert_eq!(date(2000, 2, 29, 0, 0, 0).unix_seconds(), Some(951_782_400));
        assert_eq!(date(2024, 2, 29, 23, 59, 59).unix_seconds(), Some(1_709_251_199));
        assert_eq!(date(2099, 12, 31, 23, 59, 59).unix_seconds(), Some(4_102_444_799));
    }

    #[test]
    #[cfg_attr(miri, ignore = "no unsafe in this module: Miri checks nothing the test does not (K187)")]
    fn every_day_follows_the_one_before() {
        let mut expected = 0;
        for year in 1970..2101 {
            for month in 1..=12 {
                for day in 1..=days_in_month(year, month) {
                    assert_eq!(days_from_civil(i64::from(year), month, day), expected, "{year}-{month}-{day}");
                    expected += 1;
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "no unsafe in this module: Miri checks nothing the test does not (K187)")]
    fn seconds_turn_back_into_dates() {
        assert_eq!(Date::from_unix_seconds(0), date(1970, 1, 1, 0, 0, 0));
        assert_eq!(Date::from_unix_seconds(1_795_614_330), date(2026, 11, 25, 13, 45, 30));
        assert_eq!(Date::from_unix_seconds(951_782_400), date(2000, 2, 29, 0, 0, 0));
        // Every day's first and last second, 1970 to 2100.
        for day in 0..47_847u64 {
            for seconds in [day * 86400, day * 86400 + 86399] {
                assert_eq!(Date::from_unix_seconds(seconds).unix_seconds(), Some(seconds));
            }
        }
        assert_eq!(alloc::format!("{}", date(2026, 1, 2, 3, 4, 5)), "2026-01-02 03:04:05");
    }

    #[test]
    fn dates_that_are_not() {
        assert_eq!(date(2023, 2, 29, 0, 0, 0).unix_seconds(), None);
        assert_eq!(date(1900, 2, 29, 0, 0, 0).unix_seconds(), None);
        assert_eq!(date(2026, 4, 31, 0, 0, 0).unix_seconds(), None);
        assert_eq!(date(2026, 13, 1, 0, 0, 0).unix_seconds(), None);
        assert_eq!(date(2026, 1, 0, 0, 0, 0).unix_seconds(), None);
        assert_eq!(date(2026, 1, 1, 24, 0, 0).unix_seconds(), None);
        assert_eq!(date(1969, 12, 31, 23, 59, 59).unix_seconds(), None);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
    }

    #[test]
    fn bcd() {
        assert_eq!(from_bcd(0x00), 0);
        assert_eq!(from_bcd(0x59), 59);
        assert_eq!(from_bcd(0x26), 26);
    }

    #[test]
    fn cmos_in_bcd_with_24_hours() {
        let registers =
            CmosRegisters { second: 0x30, minute: 0x45, hour: 0x13, day: 0x25, month: 0x11, year: 0x26, status_b: HOURS_24 };
        assert_eq!(registers.date(), date(2026, 11, 25, 13, 45, 30));
    }

    #[test]
    fn cmos_in_binary() {
        let registers = CmosRegisters { second: 30, minute: 45, hour: 13, day: 25, month: 11, year: 26, status_b: HOURS_24 | BINARY };
        assert_eq!(registers.date(), date(2026, 11, 25, 13, 45, 30));
    }

    #[test]
    fn cmos_with_12_hours() {
        let at = |hour: u8| CmosRegisters { second: 0, minute: 0, hour, day: 1, month: 1, year: 0x26, status_b: 0 }.date().hour;
        assert_eq!(at(0x12), 0, "12 AM is midnight");
        assert_eq!(at(0x01), 1);
        assert_eq!(at(0x11), 11);
        assert_eq!(at(PM | 0x12), 12, "12 PM is noon");
        assert_eq!(at(PM | 0x01), 13);
        assert_eq!(at(PM | 0x11), 23);
        // And in binary.
        let binary = CmosRegisters { second: 0, minute: 0, hour: PM | 1, day: 1, month: 1, year: 26, status_b: BINARY };
        assert_eq!(binary.date().hour, 13);
    }
}
