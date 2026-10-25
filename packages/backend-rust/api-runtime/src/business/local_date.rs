//! Server-local JS Date setter semantics used by native calendar operations.
use jiff::{
    Span, Timestamp, Zoned,
    civil::{Date, DateTime, Time},
    tz::{Disambiguation, TimeZone},
};

#[derive(Debug)]
pub enum CalendarError {
    UnknownTimeZone,
    DateOutOfRange,
}

pub struct BirthdayDate {
    pub text: String,
    #[cfg(test)]
    pub instant_ms: i64,
}

pub struct LocalCalendar {
    zone: TimeZone,
}

impl LocalCalendar {
    /// Receives the host's effective canonical zone, not the raw TZ setting.
    pub fn new(effective_zone: &str) -> Result<Self, CalendarError> {
        Ok(Self {
            zone: TimeZone::get(effective_zone).map_err(|_| CalendarError::UnknownTimeZone)?,
        })
    }

    pub fn birthday(
        &self,
        mmdd: i16,
        mut now: impl FnMut() -> i64,
    ) -> Result<BirthdayDate, CalendarError> {
        let first = self.instant(now())?;
        let midnight = self.set_hours(&first, 0, 0, 0, 0)?;
        // JS floor for the month and signed remainder for the day differ for
        // negative mmdd. Preserve both rather than applying calendar validity.
        let month = i32::from(mmdd).div_euclid(100) - 1;
        let day = i32::from(mmdd) % 100;
        let date = normalized_date(i32::from(midnight.year()), month, day)?;
        let mut birthday = self.resolve(DateTime::from_parts(date, midnight.time()))?;
        let second = self.instant(now())?;
        let today = self.set_hours(&second, 0, 0, 0, 0)?;
        if birthday.timestamp() < today.timestamp() {
            // This third fresh clock is conditional and can observe a new year.
            let third = self.instant(now())?;
            let year = i32::from(third.year()) + 1;
            let date = normalized_date(
                year,
                i32::from(birthday.month()) - 1,
                i32::from(birthday.day()),
            )?;
            birthday = self.resolve(DateTime::from_parts(date, birthday.time()))?;
        }
        Ok(BirthdayDate {
            text: format!(
                "{}-{:02}-{:02}",
                birthday.year(),
                birthday.month(),
                birthday.day()
            ),
            #[cfg(test)]
            instant_ms: birthday.timestamp().as_millisecond(),
        })
    }

    pub(crate) fn hashtag_windows(
        &self,
        millis: i64,
        range: usize,
    ) -> Result<Vec<String>, CalendarError> {
        let instant = self.instant(millis)?;
        let rounded = i64::from(instant.minute()).div_euclid(10) * 10;
        let mut value = self.hashtag_set_minutes(&instant, rounded)?;
        let mut windows = Vec::with_capacity(range);
        for index in 0..range {
            let utc = value.timestamp().to_zoned(TimeZone::UTC);
            windows.push(format!(
                "{}{:02}{:02}{:02}{:02}",
                utc.year(),
                utc.month(),
                utc.day(),
                utc.hour(),
                utc.minute()
            ));
            let index = i64::try_from(index).map_err(|_| CalendarError::DateOutOfRange)?;
            let offset = index.checked_mul(10).ok_or(CalendarError::DateOutOfRange)?;
            let minute = i64::from(value.minute())
                .checked_sub(offset)
                .ok_or(CalendarError::DateOutOfRange)?;
            // HashtagService.getCharts applies i*10 cumulatively AFTER emission,
            // including the i=0 no-op in civil time and the final unused setter.
            value = self.hashtag_set_minutes(&value, minute)?;
        }
        Ok(windows)
    }

    pub(crate) fn hashtag_set_minutes(
        &self,
        value: &Zoned,
        minute: i64,
    ) -> Result<Zoned, CalendarError> {
        // Date.setMinutes uses the current local date/hour, accepts minute overflow,
        // and (in this caller) replaces seconds/ms with zero. Normalize civil time
        // BEFORE resolving: instant-duration subtraction differs at gaps/folds.
        let midnight = Time::new(0, 0, 0, 0).map_err(|_| CalendarError::DateOutOfRange)?;
        let total = i64::from(value.hour())
            .checked_mul(60)
            .and_then(|hours| hours.checked_add(minute))
            .ok_or(CalendarError::DateOutOfRange)?;
        let civil = DateTime::from_parts(value.date(), midnight)
            .checked_add(Span::new().minutes(total))
            .map_err(|_| CalendarError::DateOutOfRange)?;
        self.resolve(civil)
    }

    pub(crate) fn instant(&self, millis: i64) -> Result<Zoned, CalendarError> {
        Ok(Timestamp::from_millisecond(millis)
            .map_err(|_| CalendarError::DateOutOfRange)?
            .to_zoned(self.zone.clone()))
    }

    pub(crate) fn set_hours(
        &self,
        value: &Zoned,
        hour: i8,
        minute: i8,
        second: i8,
        millis: i16,
    ) -> Result<Zoned, CalendarError> {
        let time = Time::new(hour, minute, second, i32::from(millis) * 1_000_000)
            .map_err(|_| CalendarError::DateOutOfRange)?;
        self.resolve(DateTime::from_parts(value.date(), time))
    }

    fn resolve(&self, civil: DateTime) -> Result<Zoned, CalendarError> {
        // Do not prefer a previous offset: JS uses compatible disambiguation
        // afresh for each setter, choosing earlier folds and advancing gaps.
        self.zone
            .to_ambiguous_zoned(civil)
            .disambiguate(Disambiguation::Compatible)
            .map_err(|_| CalendarError::DateOutOfRange)
    }
}

fn normalized_date(year: i32, month_zero: i32, day: i32) -> Result<Date, CalendarError> {
    let year = year
        .checked_add(month_zero.div_euclid(12))
        .ok_or(CalendarError::DateOutOfRange)?;
    let year = i16::try_from(year).map_err(|_| CalendarError::DateOutOfRange)?;
    let month =
        i8::try_from(month_zero.rem_euclid(12) + 1).map_err(|_| CalendarError::DateOutOfRange)?;
    // Start on day1, then add day-1 civil days; neither clamp nor reject Feb31.
    Date::new(year, month, 1)
        .and_then(|date| date.checked_add(Span::new().days(i64::from(day) - 1)))
        .map_err(|_| CalendarError::DateOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birthdays_match_bun_date_setter_order_and_fresh_clocks() {
        // Generated from the original Bun handler at fixed instants.
        type Fixture<'a> = (&'a str, i16, &'a [i64], &'a str, i64, usize);
        let fixtures: &[Fixture<'_>] = &[
            (
                "UTC",
                1024,
                &[1792843200000, 1792843200000, 1792843200000],
                "2026-10-24",
                1792800000000,
                2,
            ),
            (
                "UTC",
                1023,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-10-23",
                1824249600000,
                3,
            ),
            (
                "UTC",
                1025,
                &[1792843200000, 1792843200000, 1792843200000],
                "2026-10-25",
                1792886400000,
                2,
            ),
            (
                "UTC",
                1231,
                &[1792843200000, 1792843200000, 1792843200000],
                "2026-12-31",
                1798675200000,
                2,
            ),
            (
                "UTC",
                101,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-01-01",
                1798761600000,
                3,
            ),
            (
                "UTC",
                231,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-03-03",
                1804032000000,
                3,
            ),
            (
                "UTC",
                200,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-01-31",
                1801353600000,
                3,
            ),
            (
                "UTC",
                31,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-12-31",
                1830211200000,
                3,
            ),
            (
                "UTC",
                -101,
                &[1792843200000, 1792843200000, 1792843200000],
                "2027-09-29",
                1822176000000,
                3,
            ),
            (
                "UTC",
                32767,
                &[1792843200000, 1792843200000, 1792843200000],
                "2053-05-06",
                2630102400000,
                2,
            ),
            (
                "UTC",
                1231,
                &[1798718400000, 1798718400000, 1798718400000],
                "2026-12-31",
                1798675200000,
                2,
            ),
            (
                "UTC",
                101,
                &[1798718400000, 1798718400000, 1798718400000],
                "2027-01-01",
                1798761600000,
                3,
            ),
            (
                "UTC",
                1231,
                &[1767268800000, 1767268800000, 1767268800000],
                "2026-12-31",
                1798675200000,
                2,
            ),
            (
                "UTC",
                229,
                &[1677672000000, 1677672000000, 1677672000000],
                "2023-03-01",
                1677628800000,
                2,
            ),
            (
                "UTC",
                229,
                &[1677758400000, 1677758400000, 1677758400000],
                "2024-03-01",
                1709251200000,
                3,
            ),
            (
                "UTC",
                229,
                &[1709294400000, 1709294400000, 1709294400000],
                "2025-03-01",
                1740787200000,
                3,
            ),
            (
                "UTC",
                231,
                &[1704110400000, 1704110400000, 1704110400000],
                "2024-03-02",
                1709337600000,
                2,
            ),
            (
                "UTC",
                1230,
                &[1309521600000, 1309521600000, 1309521600000],
                "2011-12-30",
                1325203200000,
                2,
            ),
            (
                "UTC",
                1230,
                &[1325332800000, 1325332800000, 1325332800000],
                "2012-12-30",
                1356825600000,
                3,
            ),
            (
                "UTC",
                908,
                &[1725796800000, 1725796800000, 1725796800000],
                "2024-09-08",
                1725753600000,
                2,
            ),
            (
                "UTC",
                908,
                &[1704110400000, 1704110400000, 1704110400000],
                "2024-09-08",
                1725753600000,
                2,
            ),
            (
                "UTC",
                1024,
                &[1792886399999, 1792886400000, 1792929600000],
                "2027-10-24",
                1824336000000,
                3,
            ),
            (
                "UTC",
                1231,
                &[1798761599999, 1798761600000, 1798804800000],
                "2028-12-31",
                1861833600000,
                3,
            ),
            (
                "UTC",
                101,
                &[1798761599990, 1798761599999, 1798804800000],
                "2028-01-01",
                1830297600000,
                3,
            ),
            (
                "Asia/Tokyo",
                1024,
                &[1792810800000, 1792810800000, 1792810800000],
                "2026-10-24",
                1792767600000,
                2,
            ),
            (
                "Asia/Tokyo",
                1023,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-10-23",
                1824217200000,
                3,
            ),
            (
                "Asia/Tokyo",
                1025,
                &[1792810800000, 1792810800000, 1792810800000],
                "2026-10-25",
                1792854000000,
                2,
            ),
            (
                "Asia/Tokyo",
                1231,
                &[1792810800000, 1792810800000, 1792810800000],
                "2026-12-31",
                1798642800000,
                2,
            ),
            (
                "Asia/Tokyo",
                101,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-01-01",
                1798729200000,
                3,
            ),
            (
                "Asia/Tokyo",
                231,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-03-03",
                1803999600000,
                3,
            ),
            (
                "Asia/Tokyo",
                200,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-01-31",
                1801321200000,
                3,
            ),
            (
                "Asia/Tokyo",
                31,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-12-31",
                1830178800000,
                3,
            ),
            (
                "Asia/Tokyo",
                -101,
                &[1792810800000, 1792810800000, 1792810800000],
                "2027-09-29",
                1822143600000,
                3,
            ),
            (
                "Asia/Tokyo",
                32767,
                &[1792810800000, 1792810800000, 1792810800000],
                "2053-05-06",
                2630070000000,
                2,
            ),
            (
                "Asia/Tokyo",
                1231,
                &[1798686000000, 1798686000000, 1798686000000],
                "2026-12-31",
                1798642800000,
                2,
            ),
            (
                "Asia/Tokyo",
                101,
                &[1798686000000, 1798686000000, 1798686000000],
                "2027-01-01",
                1798729200000,
                3,
            ),
            (
                "Asia/Tokyo",
                1231,
                &[1767236400000, 1767236400000, 1767236400000],
                "2026-12-31",
                1798642800000,
                2,
            ),
            (
                "Asia/Tokyo",
                229,
                &[1677639600000, 1677639600000, 1677639600000],
                "2023-03-01",
                1677596400000,
                2,
            ),
            (
                "Asia/Tokyo",
                229,
                &[1677726000000, 1677726000000, 1677726000000],
                "2024-03-01",
                1709218800000,
                3,
            ),
            (
                "Asia/Tokyo",
                229,
                &[1709262000000, 1709262000000, 1709262000000],
                "2025-03-01",
                1740754800000,
                3,
            ),
            (
                "Asia/Tokyo",
                231,
                &[1704078000000, 1704078000000, 1704078000000],
                "2024-03-02",
                1709305200000,
                2,
            ),
            (
                "Asia/Tokyo",
                1230,
                &[1309489200000, 1309489200000, 1309489200000],
                "2011-12-30",
                1325170800000,
                2,
            ),
            (
                "Asia/Tokyo",
                1230,
                &[1325300400000, 1325300400000, 1325300400000],
                "2012-12-30",
                1356793200000,
                3,
            ),
            (
                "Asia/Tokyo",
                908,
                &[1725764400000, 1725764400000, 1725764400000],
                "2024-09-08",
                1725721200000,
                2,
            ),
            (
                "Asia/Tokyo",
                908,
                &[1704078000000, 1704078000000, 1704078000000],
                "2024-09-08",
                1725721200000,
                2,
            ),
            (
                "Asia/Tokyo",
                1024,
                &[1792853999999, 1792854000000, 1792897200000],
                "2027-10-24",
                1824303600000,
                3,
            ),
            (
                "Asia/Tokyo",
                1231,
                &[1798729199999, 1798729200000, 1798772400000],
                "2028-12-31",
                1861801200000,
                3,
            ),
            (
                "Asia/Tokyo",
                101,
                &[1798729199990, 1798729199999, 1798772400000],
                "2028-01-01",
                1830265200000,
                3,
            ),
            (
                "America/New_York",
                1024,
                &[1792857600000, 1792857600000, 1792857600000],
                "2026-10-24",
                1792814400000,
                2,
            ),
            (
                "America/New_York",
                1023,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-10-23",
                1824264000000,
                3,
            ),
            (
                "America/New_York",
                1025,
                &[1792857600000, 1792857600000, 1792857600000],
                "2026-10-25",
                1792900800000,
                2,
            ),
            (
                "America/New_York",
                1231,
                &[1792857600000, 1792857600000, 1792857600000],
                "2026-12-31",
                1798693200000,
                2,
            ),
            (
                "America/New_York",
                101,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-01-01",
                1798779600000,
                3,
            ),
            (
                "America/New_York",
                231,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-03-03",
                1804050000000,
                3,
            ),
            (
                "America/New_York",
                200,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-01-31",
                1801371600000,
                3,
            ),
            (
                "America/New_York",
                31,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-12-31",
                1830229200000,
                3,
            ),
            (
                "America/New_York",
                -101,
                &[1792857600000, 1792857600000, 1792857600000],
                "2027-09-29",
                1822190400000,
                3,
            ),
            (
                "America/New_York",
                32767,
                &[1792857600000, 1792857600000, 1792857600000],
                "2053-05-06",
                2630116800000,
                2,
            ),
            (
                "America/New_York",
                1231,
                &[1798736400000, 1798736400000, 1798736400000],
                "2026-12-31",
                1798693200000,
                2,
            ),
            (
                "America/New_York",
                101,
                &[1798736400000, 1798736400000, 1798736400000],
                "2027-01-01",
                1798779600000,
                3,
            ),
            (
                "America/New_York",
                1231,
                &[1767286800000, 1767286800000, 1767286800000],
                "2026-12-31",
                1798693200000,
                2,
            ),
            (
                "America/New_York",
                229,
                &[1677690000000, 1677690000000, 1677690000000],
                "2023-03-01",
                1677646800000,
                2,
            ),
            (
                "America/New_York",
                229,
                &[1677776400000, 1677776400000, 1677776400000],
                "2024-03-01",
                1709269200000,
                3,
            ),
            (
                "America/New_York",
                229,
                &[1709312400000, 1709312400000, 1709312400000],
                "2025-03-01",
                1740805200000,
                3,
            ),
            (
                "America/New_York",
                231,
                &[1704128400000, 1704128400000, 1704128400000],
                "2024-03-02",
                1709355600000,
                2,
            ),
            (
                "America/New_York",
                1230,
                &[1309536000000, 1309536000000, 1309536000000],
                "2011-12-30",
                1325221200000,
                2,
            ),
            (
                "America/New_York",
                1230,
                &[1325350800000, 1325350800000, 1325350800000],
                "2012-12-30",
                1356843600000,
                3,
            ),
            (
                "America/New_York",
                908,
                &[1725811200000, 1725811200000, 1725811200000],
                "2024-09-08",
                1725768000000,
                2,
            ),
            (
                "America/New_York",
                908,
                &[1704128400000, 1704128400000, 1704128400000],
                "2024-09-08",
                1725768000000,
                2,
            ),
            (
                "America/New_York",
                1024,
                &[1792900799999, 1792900800000, 1792944000000],
                "2027-10-24",
                1824350400000,
                3,
            ),
            (
                "America/New_York",
                1231,
                &[1798779599999, 1798779600000, 1798822800000],
                "2028-12-31",
                1861851600000,
                3,
            ),
            (
                "America/New_York",
                101,
                &[1798779599990, 1798779599999, 1798822800000],
                "2028-01-01",
                1830315600000,
                3,
            ),
            (
                "Pacific/Apia",
                1024,
                &[1792796400000, 1792796400000, 1792796400000],
                "2026-10-24",
                1792753200000,
                2,
            ),
            (
                "Pacific/Apia",
                1023,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-10-23",
                1824202800000,
                3,
            ),
            (
                "Pacific/Apia",
                1025,
                &[1792796400000, 1792796400000, 1792796400000],
                "2026-10-25",
                1792839600000,
                2,
            ),
            (
                "Pacific/Apia",
                1231,
                &[1792796400000, 1792796400000, 1792796400000],
                "2026-12-31",
                1798628400000,
                2,
            ),
            (
                "Pacific/Apia",
                101,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-01-01",
                1798714800000,
                3,
            ),
            (
                "Pacific/Apia",
                231,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-03-03",
                1803985200000,
                3,
            ),
            (
                "Pacific/Apia",
                200,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-01-31",
                1801306800000,
                3,
            ),
            (
                "Pacific/Apia",
                31,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-12-31",
                1830164400000,
                3,
            ),
            (
                "Pacific/Apia",
                -101,
                &[1792796400000, 1792796400000, 1792796400000],
                "2027-09-29",
                1822129200000,
                3,
            ),
            (
                "Pacific/Apia",
                32767,
                &[1792796400000, 1792796400000, 1792796400000],
                "2053-05-06",
                2630055600000,
                2,
            ),
            (
                "Pacific/Apia",
                1231,
                &[1798671600000, 1798671600000, 1798671600000],
                "2026-12-31",
                1798628400000,
                2,
            ),
            (
                "Pacific/Apia",
                101,
                &[1798671600000, 1798671600000, 1798671600000],
                "2027-01-01",
                1798714800000,
                3,
            ),
            (
                "Pacific/Apia",
                1231,
                &[1767222000000, 1767222000000, 1767222000000],
                "2026-12-31",
                1798628400000,
                2,
            ),
            (
                "Pacific/Apia",
                229,
                &[1677625200000, 1677625200000, 1677625200000],
                "2023-03-01",
                1677582000000,
                2,
            ),
            (
                "Pacific/Apia",
                229,
                &[1677711600000, 1677711600000, 1677711600000],
                "2024-03-01",
                1709204400000,
                3,
            ),
            (
                "Pacific/Apia",
                229,
                &[1709247600000, 1709247600000, 1709247600000],
                "2025-03-01",
                1740740400000,
                3,
            ),
            (
                "Pacific/Apia",
                231,
                &[1704063600000, 1704063600000, 1704063600000],
                "2024-03-02",
                1709290800000,
                2,
            ),
            (
                "Pacific/Apia",
                1230,
                &[1309561200000, 1309561200000, 1309561200000],
                "2011-12-31",
                1325239200000,
                2,
            ),
            (
                "Pacific/Apia",
                1230,
                &[1325282400000, 1325282400000, 1325282400000],
                "2011-12-31",
                1325239200000,
                2,
            ),
            (
                "Pacific/Apia",
                908,
                &[1725750000000, 1725750000000, 1725750000000],
                "2024-09-08",
                1725706800000,
                2,
            ),
            (
                "Pacific/Apia",
                908,
                &[1704063600000, 1704063600000, 1704063600000],
                "2024-09-08",
                1725706800000,
                2,
            ),
            (
                "Pacific/Apia",
                1024,
                &[1792839599999, 1792839600000, 1792882800000],
                "2027-10-24",
                1824289200000,
                3,
            ),
            (
                "Pacific/Apia",
                1231,
                &[1798714799999, 1798714800000, 1798758000000],
                "2028-12-31",
                1861786800000,
                3,
            ),
            (
                "Pacific/Apia",
                101,
                &[1798714799990, 1798714799999, 1798758000000],
                "2028-01-01",
                1830250800000,
                3,
            ),
            (
                "America/Santiago",
                1024,
                &[1792854000000, 1792854000000, 1792854000000],
                "2026-10-24",
                1792810800000,
                2,
            ),
            (
                "America/Santiago",
                1023,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-10-23",
                1824260400000,
                3,
            ),
            (
                "America/Santiago",
                1025,
                &[1792854000000, 1792854000000, 1792854000000],
                "2026-10-25",
                1792897200000,
                2,
            ),
            (
                "America/Santiago",
                1231,
                &[1792854000000, 1792854000000, 1792854000000],
                "2026-12-31",
                1798686000000,
                2,
            ),
            (
                "America/Santiago",
                101,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-01-01",
                1798772400000,
                3,
            ),
            (
                "America/Santiago",
                231,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-03-03",
                1804042800000,
                3,
            ),
            (
                "America/Santiago",
                200,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-01-31",
                1801364400000,
                3,
            ),
            (
                "America/Santiago",
                31,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-12-31",
                1830222000000,
                3,
            ),
            (
                "America/Santiago",
                -101,
                &[1792854000000, 1792854000000, 1792854000000],
                "2027-09-29",
                1822186800000,
                3,
            ),
            (
                "America/Santiago",
                32767,
                &[1792854000000, 1792854000000, 1792854000000],
                "2053-05-06",
                2630116800000,
                2,
            ),
            (
                "America/Santiago",
                1231,
                &[1798729200000, 1798729200000, 1798729200000],
                "2026-12-31",
                1798686000000,
                2,
            ),
            (
                "America/Santiago",
                101,
                &[1798729200000, 1798729200000, 1798729200000],
                "2027-01-01",
                1798772400000,
                3,
            ),
            (
                "America/Santiago",
                1231,
                &[1767279600000, 1767279600000, 1767279600000],
                "2026-12-31",
                1798686000000,
                2,
            ),
            (
                "America/Santiago",
                229,
                &[1677682800000, 1677682800000, 1677682800000],
                "2023-03-01",
                1677639600000,
                2,
            ),
            (
                "America/Santiago",
                229,
                &[1677769200000, 1677769200000, 1677769200000],
                "2024-03-01",
                1709262000000,
                3,
            ),
            (
                "America/Santiago",
                229,
                &[1709305200000, 1709305200000, 1709305200000],
                "2025-03-01",
                1740798000000,
                3,
            ),
            (
                "America/Santiago",
                231,
                &[1704121200000, 1704121200000, 1704121200000],
                "2024-03-02",
                1709348400000,
                2,
            ),
            (
                "America/Santiago",
                1230,
                &[1309536000000, 1309536000000, 1309536000000],
                "2011-12-30",
                1325214000000,
                2,
            ),
            (
                "America/Santiago",
                1230,
                &[1325343600000, 1325343600000, 1325343600000],
                "2012-12-30",
                1356836400000,
                3,
            ),
            (
                "America/Santiago",
                908,
                &[1725807600000, 1725807600000, 1725807600000],
                "2024-09-08",
                1725768000000,
                2,
            ),
            (
                "America/Santiago",
                908,
                &[1704121200000, 1704121200000, 1704121200000],
                "2024-09-08",
                1725768000000,
                2,
            ),
            (
                "America/Santiago",
                1024,
                &[1792897199999, 1792897200000, 1792940400000],
                "2027-10-24",
                1824346800000,
                3,
            ),
            (
                "America/Santiago",
                1231,
                &[1798772399999, 1798772400000, 1798815600000],
                "2028-12-31",
                1861844400000,
                3,
            ),
            (
                "America/Santiago",
                101,
                &[1798772399990, 1798772399999, 1798815600000],
                "2028-01-01",
                1830308400000,
                3,
            ),
        ];
        for (zone, mmdd, times, expected, instant, count) in fixtures {
            let calendar = LocalCalendar::new(zone).unwrap();
            let mut reads = 0;
            let value = calendar
                .birthday(*mmdd, || {
                    let ms = times[reads];
                    reads += 1;
                    ms
                })
                .unwrap();
            assert_eq!(value.text, *expected, "{zone}/{mmdd}");
            assert_eq!(value.instant_ms, *instant, "{zone}/{mmdd}");
            assert_eq!(reads, *count, "{zone}/{mmdd}");
        }
    }

    #[test]
    fn compatible_setters_match_bun_gap_and_fold_instants() {
        let fixtures: &[(&str, i64, i8, i8, i64)] = &[
            ("UTC", 1710072000000, 2, 30, 1710037800000),
            ("UTC", 1730635200000, 1, 30, 1730597400000),
            ("UTC", 1325246400000, 0, 0, 1325203200000),
            ("UTC", 1725796800000, 0, 0, 1725753600000),
            ("Asia/Tokyo", 1710039600000, 2, 30, 1710005400000),
            ("Asia/Tokyo", 1730602800000, 1, 30, 1730565000000),
            ("Asia/Tokyo", 1325214000000, 0, 0, 1325170800000),
            ("Asia/Tokyo", 1725764400000, 0, 0, 1725721200000),
            ("America/New_York", 1710086400000, 2, 30, 1710055800000),
            ("America/New_York", 1730653200000, 1, 30, 1730611800000),
            ("America/New_York", 1325264400000, 0, 0, 1325221200000),
            ("America/New_York", 1725811200000, 0, 0, 1725768000000),
            ("Pacific/Apia", 1710025200000, 2, 30, 1709991000000),
            ("Pacific/Apia", 1730588400000, 1, 30, 1730550600000),
            ("Pacific/Apia", 1325282400000, 0, 0, 1325239200000),
            ("Pacific/Apia", 1725750000000, 0, 0, 1725706800000),
            ("America/Santiago", 1710082800000, 2, 30, 1710048600000),
            ("America/Santiago", 1730646000000, 1, 30, 1730608200000),
            ("America/Santiago", 1325257200000, 0, 0, 1325214000000),
            ("America/Santiago", 1725807600000, 0, 0, 1725768000000),
        ];
        for (zone, before, hour, minute, expected) in fixtures {
            let calendar = LocalCalendar::new(zone).unwrap();
            let before = calendar.instant(*before).unwrap();
            let after = calendar.set_hours(&before, *hour, *minute, 0, 0).unwrap();
            assert_eq!(after.timestamp().as_millisecond(), *expected, "{zone}");
        }
    }

    #[test]
    fn uses_explicit_effective_zone_and_rejects_unknown_identifiers() {
        assert!(LocalCalendar::new("must-not-exist.invalid").is_err());
        for zone in [
            "UTC",
            "Asia/Tokyo",
            "America/New_York",
            "Pacific/Apia",
            "America/Santiago",
        ] {
            assert!(LocalCalendar::new(zone).is_ok());
        }
    }
}
