//! Shared IdService parsing and ECMAScript dates for native read operations.
use super::Error;

/// ECMAScript Date.toISOString, including TimeClip and expanded years. PG Date
/// hydration has already rounded each timestamp down to whole milliseconds.
pub(super) fn iso_date(milliseconds: f64) -> Result<String, Error> {
    if !milliseconds.is_finite() || milliseconds.abs() > 8_640_000_000_000_000.0 {
        return Err(Error::Internal);
    }
    let milliseconds = milliseconds.trunc() as i64;
    let days = milliseconds.div_euclid(86_400_000);
    let time = milliseconds.rem_euclid(86_400_000);
    // Gregorian civil date from days since 1970-01-01 (400-year eras).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!(
            "{}{year:06}",
            if year < 0 { "-" } else { "+" },
            year = year.abs()
        )
    };
    Ok(format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000,
    ))
}

/// IdService.parse uses parseInt on a UTF-16 slice without validating the ID.
/// ECMAScript whitespace excludes U+0085, which Rust's is_whitespace includes.
pub(super) fn parse_id_date(method: &str, id: &str) -> Option<f64> {
    let units: Vec<_> = id.encode_utf16().collect();
    let prefix = |start: usize, len: usize, radix: u32| {
        parse_int(
            &units[start.min(units.len())..(start + len).min(units.len())],
            radix,
        )
    };
    let date = match method {
        "aid" | "aidx" => prefix(0, 8, 36)? + 946_684_800_000.0,
        "meid" => prefix(0, 12, 16)? - 140_737_488_355_328.0,
        "meidg" => prefix(1, 11, 16)?,
        "objectid" => prefix(0, 8, 16)? * 1000.0,
        "ulid" => {
            let mut result = 0.0;
            for unit in units.iter().take(10) {
                let digit = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"
                    .iter()
                    .position(|c| u16::from(*c) == *unit);
                result = result * 32.0 + digit.map_or(-1.0, |d| d as f64);
            }
            result
        }
        _ => return None,
    };
    (date.is_finite() && date.abs() <= 8_640_000_000_000_000.0).then(|| date.trunc())
}

fn parse_int(units: &[u16], radix: u32) -> Option<f64> {
    let whitespace = |unit: &u16| matches!(*unit, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff);
    let mut units = &units[units.iter().take_while(|unit| whitespace(unit)).count()..];
    let negative = match units.first() {
        Some(0x2d) => {
            units = &units[1..];
            true
        }
        Some(0x2b) => {
            units = &units[1..];
            false
        }
        _ => false,
    };
    if radix == 16 && units.first() == Some(&0x30) && matches!(units.get(1), Some(0x78 | 0x58)) {
        units = &units[2..];
    }
    let mut seen = false;
    let mut result = 0.0;
    for unit in units {
        let digit = match *unit {
            0x30..=0x39 => u32::from(*unit - 0x30),
            0x41..=0x5a => u32::from(*unit - 0x41) + 10,
            0x61..=0x7a => u32::from(*unit - 0x61) + 10,
            _ => break,
        };
        if digit >= radix {
            break;
        }
        result = result * f64::from(radix) + f64::from(digit);
        seen = true;
    }
    seen.then_some(if negative { -result } else { result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_iso_matches_timeclip_epoch_leap_days_and_expanded_years() {
        assert_eq!(iso_date(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_date(-1.9).unwrap(), "1969-12-31T23:59:59.999Z");
        assert_eq!(
            iso_date(951_782_400_000.0).unwrap(),
            "2000-02-29T00:00:00.000Z"
        );
        assert_eq!(
            iso_date(-62_167_219_200_000.0).unwrap(),
            "0000-01-01T00:00:00.000Z"
        );
        assert_eq!(
            iso_date(-62_198_755_200_000.0).unwrap(),
            "-000001-01-01T00:00:00.000Z"
        );
        assert_eq!(
            iso_date(8_640_000_000_000_000.0).unwrap(),
            "+275760-09-13T00:00:00.000Z"
        );
        assert_eq!(
            iso_date(-8_640_000_000_000_000.0).unwrap(),
            "-271821-04-20T00:00:00.000Z"
        );
        assert!(iso_date(f64::INFINITY).is_err());
        assert!(iso_date(8_640_000_000_000_001.0).is_err());
    }

    #[test]
    fn id_parser_keeps_utf16_slicing_ascii_digits_and_ecmascript_whitespace() {
        assert_eq!(parse_id_date("aid", "\u{85}1"), None);
        assert_eq!(parse_id_date("aid", "\u{a0}1"), Some(946_684_800_001.0));
        assert_eq!(parse_id_date("aid", "\u{feff}+1x"), Some(946_684_800_069.0));
        assert_eq!(
            parse_id_date("meid", "0x10tail"),
            Some(16.0 - 140_737_488_355_328.0)
        );
        assert_eq!(parse_id_date("objectid", "-1tail"), Some(-1000.0));
        assert_eq!(parse_id_date("ulid", ""), Some(0.0));
        assert_eq!(parse_id_date("ulid", "a"), Some(-1.0));
        assert_eq!(parse_id_date("aid", "😀12345678"), None);
    }
}
