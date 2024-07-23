/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around download statistics

use byteorder::ByteOrder;
use chrono::{Datelike, Days, Local, NaiveDate};
use semver::Version;
use serde_derive::Serialize;

/// The length of a series, i.e. the maximum number of days in the series
pub const SERIES_LENGTH: usize = 90;

/// The download counters for a specific version
#[derive(Debug, Clone, Serialize)]
pub struct DownloadStatsForVersion {
    /// The version
    pub version: String,
    /// The parsed semver version
    #[serde(skip)]
    version_semver: Version,
    /// The download counts for each day
    pub counts: Vec<u32>,
    /// The total for the series
    pub total: u32,
}

/// The download stats for a crate, for each version
#[derive(Debug, Clone, Serialize)]
pub struct DownloadStats {
    /// The days in the data series
    pub days: Vec<NaiveDate>,
    /// The stats for each version
    pub versions: Vec<DownloadStatsForVersion>,
}

impl DownloadStats {
    /// Creates stats with initialized dates
    #[must_use]
    pub fn new() -> Self {
        let today = Local::now().naive_local().date();
        let first = today.checked_sub_days(Days::new(SERIES_LENGTH as u64 - 1)).unwrap();
        let mut days = Vec::with_capacity(SERIES_LENGTH);
        let mut current = first;
        for _ in 0..SERIES_LENGTH {
            days.push(current);
            current = current.succ_opt().unwrap();
        }

        Self {
            days,
            versions: Vec::new(),
        }
    }

    /// Adds the data for a version
    pub fn add_version(&mut self, version: String, data: Option<&[u8]>) {
        let mut counts = vec![0; SERIES_LENGTH];
        let mut total = 0;
        if let Some(data) = data {
            let today = Local::now().naive_local().date();
            let mut index = ((today.ordinal0() + 1) as usize % SERIES_LENGTH) * std::mem::size_of::<u32>();
            for count in &mut counts {
                let v = byteorder::NativeEndian::read_u32(&data[index..]);
                total += v;
                *count = v;
                index = (index + std::mem::size_of::<u32>()) % data.len();
            }
        }
        self.versions.push(DownloadStatsForVersion {
            version_semver: version.parse().unwrap(),
            version,
            counts,
            total,
        });
    }

    /// Finalise the data by only keeping the most active versions
    pub fn finalize(&mut self) {
        self.versions.sort_unstable_by(|a, b| b.version_semver.cmp(&a.version_semver));
        let other = 4;
        if self.versions.len() > other {
            // collaped all remaining version into one
            self.versions[other].version = String::from("Others");
            for i in (other + 1)..self.versions.len() {
                self.versions[other].total += self.versions[i].total;
                for j in 0..SERIES_LENGTH {
                    self.versions[other].counts[j] += self.versions[i].counts[j];
                }
            }
            self.versions.truncate(other + 1);
        }
    }
}

#[test]
fn test_dl_stats() {
    let stats = DownloadStats::new();
    println!("{stats:?}");
}
