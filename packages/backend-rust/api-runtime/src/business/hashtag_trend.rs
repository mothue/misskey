/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete hashtag trend reads, preserving ranking order and local chart setters.
use super::{
    Error,
    local_date::{CalendarError, LocalCalendar},
};
use crate::{
    Runtime,
    redis::{HashtagBatch, HashtagCount, HashtagReadFailure, RedisState},
};
use misskey_api_validation::Value;
use std::{collections::HashSet, future::Future};

const FEATURED_EPOCH_MS: i64 = 1_672_531_200_000;
const HOUR_MS: i64 = 3_600_000;
const CHART_RANGE: usize = 20;

#[derive(Debug)]
pub(crate) enum TrendFailure {
    RankingClockOverflow,
    Calendar(CalendarError),
    Redis(HashtagReadFailure),
    CountShape { expected: usize, actual: usize },
}

impl TrendFailure {
    pub(super) fn report(&self) {
        match self {
            Self::CountShape { expected, actual } => tracing::error!(
                operation = "read_hashtags_trend",
                error.kind = self.safe_kind(),
                expected_count = *expected,
                actual_count = *actual,
                "endpoint failed"
            ),
            _ => tracing::error!(
                operation = "read_hashtags_trend",
                error.kind = self.safe_kind(),
                "endpoint failed"
            ),
        }
    }

    pub(super) fn safe_kind(&self) -> &'static str {
        match self {
            Self::RankingClockOverflow => "ranking_clock_overflow",
            Self::Calendar(CalendarError::UnknownTimeZone) => "calendar_unknown_zone",
            Self::Calendar(CalendarError::DateOutOfRange) => "calendar_date_range",
            Self::Redis(source) => source.safe_kind(),
            Self::CountShape { .. } => "chart_count_shape",
        }
    }
}

trait TrendStore {
    fn ranking<'a>(
        &'a self,
        keys: &'a [String; 2],
    ) -> impl Future<Output = Result<HashtagBatch<[Vec<String>; 2]>, HashtagReadFailure>> + Send + 'a;
    fn counts<'a>(
        &'a self,
        keys: &'a [String],
    ) -> impl Future<Output = Result<HashtagBatch<Vec<HashtagCount>>, HashtagReadFailure>> + Send + 'a;
}

impl TrendStore for RedisState {
    fn ranking<'a>(
        &'a self,
        keys: &'a [String; 2],
    ) -> impl Future<Output = Result<HashtagBatch<[Vec<String>; 2]>, HashtagReadFailure>> + Send + 'a
    {
        self.hashtag_rankings(keys)
    }
    fn counts<'a>(
        &'a self,
        keys: &'a [String],
    ) -> impl Future<Output = Result<HashtagBatch<Vec<HashtagCount>>, HashtagReadFailure>> + Send + 'a
    {
        self.hashtag_counts(keys)
    }
}

fn ranking_keys(now_ms: i64) -> Result<[String; 2], TrendFailure> {
    // FeaturedService.getCurrentWindow floors, including instants before its epoch.
    let elapsed = now_ms
        .checked_sub(FEATURED_EPOCH_MS)
        .ok_or(TrendFailure::RankingClockOverflow)?;
    let current = elapsed.div_euclid(HOUR_MS);
    let previous = current
        .checked_sub(1)
        .ok_or(TrendFailure::RankingClockOverflow)?;
    Ok([
        format!("featuredHashtagsRanking:{current}"),
        format!("featuredHashtagsRanking:{previous}"),
    ])
}

fn first_seen_tags(windows: &[Vec<String>; 2]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    // FeaturedService.getRankingOf's Map keys retain first insertion order.
    // Scores only update values that are never read by getHashtagsRanking.
    // Both insert/average branches preserve keys and first-insertion position.
    for tag in windows.iter().flatten() {
        if seen.insert(tag.as_str()) {
            tags.push(tag.clone());
        }
    }
    tags
}

fn chart_keys(tags: &[String], windows: &[String]) -> Vec<String> {
    let mut keys = Vec::with_capacity(tags.len() * windows.len());
    // HashtagService.getCharts: time-major, ranking-tag order within each time.
    for window in windows {
        for tag in tags {
            keys.push(format!("hashtagUsers:{tag}:{window}"));
        }
    }
    keys
}

fn stats(tags: &[String], counts: &[HashtagCount]) -> Result<Value, TrendFailure> {
    let expected = tags.len() * CHART_RANGE;
    if counts.len() != expected {
        return Err(TrendFailure::CountShape {
            expected,
            actual: counts.len(),
        });
    }
    let mut output = Vec::with_capacity(tags.len());
    for (tag_index, tag) in tags.iter().enumerate() {
        let mut chart = Vec::with_capacity(CHART_RANGE);
        let mut max = f64::NEG_INFINITY;
        for time_index in 0..CHART_RANGE {
            let count = match counts[time_index * tags.len() + tag_index] {
                HashtagCount::Number(number) => {
                    chart.push(Value::Number(number));
                    number
                }
                HashtagCount::Undefined => {
                    chart.push(Value::Undefined);
                    f64::NAN
                }
            };
            // Math.max propagates NaN and prefers +0 over -0.
            if count.is_nan() || max.is_nan() {
                max = f64::NAN;
            } else if count > max || (count == 0. && max == 0. && !count.is_sign_negative()) {
                max = count;
            }
        }
        // The TS intermediate {} has special __proto__/integer-key behavior,
        // but its final ranking.map reads every chart by the unchanged tag.
        // Direct vectors preserve that observable response without object-key loss.
        output.push(Value::object([
            ("tag", Value::string(tag)),
            ("chart", Value::Array(chart)),
            ("usersCount", Value::Number(max)),
        ]));
    }
    Ok(Value::Array(output))
}

async fn run(
    store: &(impl TrendStore + Sync),
    calendar: &LocalCalendar,
    mut now: impl FnMut() -> i64,
) -> Result<Value, TrendFailure> {
    let keys = ranking_keys(now())?; // First fresh clock, exactly once.
    let ranking = store.ranking(&keys).await.map_err(TrendFailure::Redis)?;
    ranking.report();
    let tags = first_seen_tags(&ranking.values);
    if tags.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    // Fresh chart clock occurs only after ranking I/O, never reused from ranking.
    let windows = calendar
        .hashtag_windows(now(), CHART_RANGE)
        .map_err(TrendFailure::Calendar)?;
    let keys = chart_keys(&tags, &windows);
    let counts = store.counts(&keys).await.map_err(TrendFailure::Redis)?;
    counts.report();
    stats(&tags, &counts.values)
}

pub(super) async fn trend(runtime: &Runtime) -> Result<Value, Error> {
    run(
        &runtime.redis,
        &runtime.business_config.local_calendar,
        crate::now_ms,
    )
    .await
    .map_err(Error::HashtagTrend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use std::{future::ready, sync::Mutex};

    // Generated from the actual TS service methods and endpoint callback.
    const SOURCE_ORACLE: &str = include_str!("fixtures/hashtag-trend-source-oracle.json");
    const ERROR_ORACLE: &str = include_str!("fixtures/hashtag-trend-errors-oracle.json");

    #[derive(Debug, PartialEq)]
    enum Call {
        Ranking([String; 2]),
        Counts(Vec<String>),
    }
    struct FakeStore<'a> {
        fixture: &'a Json,
        calls: Mutex<Vec<Call>>,
    }
    impl TrendStore for FakeStore<'_> {
        fn ranking<'a>(
            &'a self,
            keys: &'a [String; 2],
        ) -> impl Future<Output = Result<HashtagBatch<[Vec<String>; 2]>, HashtagReadFailure>> + Send + 'a
        {
            self.calls.lock().unwrap().push(Call::Ranking(keys.clone()));
            let members = |name: &str| {
                self.fixture["input"][name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| pair[0].as_str().unwrap().to_owned())
                    .collect()
            };
            ready(Ok(HashtagBatch {
                values: if self.fixture["input"]["rankingNull"] == true {
                    [Vec::new(), Vec::new()]
                } else if self.fixture["input"]["rankingErrorCurrent"] == true {
                    [Vec::new(), members("previous")]
                } else {
                    [members("current"), members("previous")]
                },
                command_errors: Vec::new(),
            }))
        }
        fn counts<'a>(
            &'a self,
            keys: &'a [String],
        ) -> impl Future<Output = Result<HashtagBatch<Vec<HashtagCount>>, HashtagReadFailure>> + Send + 'a
        {
            self.calls.lock().unwrap().push(Call::Counts(keys.to_vec()));
            ready(Ok(HashtagBatch {
                values: self.fixture["chartResponse"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|count| {
                        count
                            .as_f64()
                            .map_or(HashtagCount::Undefined, HashtagCount::Number)
                    })
                    .collect(),
                command_errors: Vec::new(),
            }))
        }
    }

    fn expected_calls(fixture: &Json) -> Vec<Call> {
        let ranking: Vec<String> = fixture["trace"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["command"][0] == "zrange")
            .map(|entry| entry["command"][1].as_str().unwrap().to_owned())
            .collect();
        let mut calls = vec![Call::Ranking(ranking.try_into().unwrap())];
        let chart: Vec<String> = fixture["trace"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["command"][0] == "pfcount")
            .map(|entry| entry["command"][1].as_str().unwrap().to_owned())
            .collect();
        if !chart.is_empty() {
            calls.push(Call::Counts(chart));
        }
        calls
    }

    #[tokio::test]
    async fn all_source_generated_successes_match_response_clocks_and_command_keys() {
        for text in [SOURCE_ORACLE, ERROR_ORACLE] {
            let oracle: Json = serde_json::from_str(text).unwrap();
            let mut compared = 0;
            for fixture in oracle["cases"].as_array().unwrap() {
                let store = FakeStore {
                    fixture,
                    calls: Mutex::new(Vec::new()),
                };
                let calendar = LocalCalendar::new(fixture["zone"].as_str().unwrap()).unwrap();
                let clocks = fixture["clockReads"].as_array().unwrap();
                let mut reads = 0;
                let actual = run(&store, &calendar, || {
                    let value = clocks[reads].as_i64().unwrap();
                    reads += 1;
                    value
                })
                .await
                .unwrap();
                assert_eq!(
                    actual.stringify().as_deref(),
                    fixture["jsonBody"].as_str(),
                    "{}",
                    fixture["name"]
                );
                assert_eq!(reads, clocks.len(), "clock count: {}", fixture["name"]);
                assert_eq!(
                    *store.calls.lock().unwrap(),
                    expected_calls(fixture),
                    "commands: {}",
                    fixture["name"]
                );
                // Borrow because public Value has a custom iterative Drop.
                let Value::Array(rows) = &actual else {
                    panic!("expected stats array")
                };
                let tags: Vec<String> = rows
                    .iter()
                    .map(|row| row.get("tag").unwrap().to_utf8().unwrap())
                    .collect();
                let source_tags: Vec<&str> = fixture["observed"]["ranking"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tag| tag.as_str().unwrap())
                    .collect();
                assert_eq!(
                    tags, source_tags,
                    "Map/prototype/numeric order: {}",
                    fixture["name"]
                );
                compared += 1;
            }
            assert_eq!(compared, oracle["summary"]["success"].as_u64().unwrap());
        }
    }

    #[test]
    fn every_source_generated_chart_window_and_setter_step_matches_local_calendar() {
        let oracle: Json = serde_json::from_str(SOURCE_ORACLE).unwrap();
        for fixture in oracle["cases"].as_array().unwrap() {
            if fixture["setters"].as_array().unwrap().is_empty() {
                continue;
            }
            let calendar = LocalCalendar::new(fixture["zone"].as_str().unwrap()).unwrap();
            let clock = fixture["clockReads"][1].as_i64().unwrap();
            let generated = calendar.hashtag_windows(clock, CHART_RANGE).unwrap();
            let windows: Vec<&str> = fixture["windows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect();
            assert_eq!(generated, windows, "{}", fixture["name"]);
            for setter in fixture["setters"].as_array().unwrap() {
                let before: jiff::Timestamp = setter["before"].as_str().unwrap().parse().unwrap();
                let expected: jiff::Timestamp = setter["after"].as_str().unwrap().parse().unwrap();
                let before = calendar.instant(before.as_millisecond()).unwrap();
                let result = calendar
                    .hashtag_set_minutes(&before, setter["arguments"][0].as_i64().unwrap())
                    .unwrap();
                assert_eq!(result.timestamp(), expected, "setter {}", fixture["name"]);
            }
        }
    }
}
