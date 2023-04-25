// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use std::cmp::Ordering;

/// The version of rustc/cargo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
	pub major: u32,
	pub minor: u32,
	pub patch: u32,
	pub is_nightly: bool,
	pub year: u32,
	pub month: u32,
	pub day: u32,
}

impl Version {
	/// Returns if `self` is a stable version.
	pub fn is_stable(&self) -> bool {
		!self.is_nightly
	}

	/// Return if `self` is a nightly version.
	pub fn is_nightly(&self) -> bool {
		self.is_nightly
	}

	/// Extract from the given `version` string.
	pub fn extract(version: &str) -> Option<Self> {
		let mut is_nightly = false;
		let version_parts = version
			.trim()
			.split(" ")
			.nth(1)?
			.split(".")
			.filter_map(|v| {
				if let Some(rest) = v.strip_suffix("-nightly") {
					is_nightly = true;
					rest.parse().ok()
				} else {
					v.parse().ok()
				}
			})
			.collect::<Vec<u32>>();

		if version_parts.len() != 3 {
			return None
		}

		let date = version.split(" ").nth(3)?;

		let date_parts = date
			.split("-")
			.filter_map(|v| v.trim().strip_suffix(")").unwrap_or(v).parse().ok())
			.collect::<Vec<u32>>();

		if date_parts.len() != 3 {
			return None
		}

		Some(Version {
			major: version_parts[0],
			minor: version_parts[1],
			patch: version_parts[2],
			is_nightly,
			year: date_parts[0],
			month: date_parts[1],
			day: date_parts[2],
		})
	}
}

/// Ordering is done in the following way:
///
/// 1. `stable` > `nightly`
/// 2. Then compare major, minor and patch.
/// 3. Last compare the date.
impl Ord for Version {
	fn cmp(&self, other: &Self) -> Ordering {
		if self == other {
			return Ordering::Equal
		}

		// Ensure that `stable > nightly`
		if self.is_stable() && other.is_nightly() {
			return Ordering::Greater
		} else if self.is_nightly() && other.is_stable() {
			return Ordering::Less
		}

		let to_compare = [
			(self.major, other.major),
			(self.minor, other.minor),
			(self.patch, other.patch),
			(self.year, other.year),
			(self.month, other.month),
			(self.day, other.day),
		];

		to_compare
			.iter()
			.find_map(|(l, r)| if l != r { l.partial_cmp(&r) } else { None })
			// We already checked this right at the beginning, so we should never return here
			// `Equal`.
			.unwrap_or(Ordering::Equal)
	}
}

impl PartialOrd for Version {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn version_compare_and_extract_works() {
		let version_1_66_0 = Version::extract("cargo 1.66.0 (d65d197ad 2022-11-15)").unwrap();
		let version_1_66_1 = Version::extract("cargo 1.66.1 (d65d197ad 2022-11-15)").unwrap();
		let version_1_66_0_nightly =
			Version::extract("cargo 1.66.0-nightly (d65d197ad 2022-10-15)").unwrap();
		let version_1_66_0_nightly_older_date =
			Version::extract("cargo 1.66.0-nightly (d65d197ad 2022-10-14)").unwrap();
		let version_1_65_0 = Version::extract("cargo 1.65.0 (d65d197ad 2022-10-15)").unwrap();
		let version_1_65_0_older_date =
			Version::extract("cargo 1.65.0 (d65d197ad 2022-10-14)").unwrap();

		assert!(version_1_66_1 > version_1_66_0);
		assert!(version_1_66_1 > version_1_65_0);
		assert!(version_1_66_1 > version_1_66_0_nightly);
		assert!(version_1_66_1 > version_1_66_0_nightly_older_date);
		assert!(version_1_66_1 > version_1_65_0_older_date);

		assert!(version_1_66_0 > version_1_65_0);
		assert!(version_1_66_0 > version_1_66_0_nightly);
		assert!(version_1_66_0 > version_1_66_0_nightly_older_date);
		assert!(version_1_66_0 > version_1_65_0_older_date);

		assert!(version_1_65_0 > version_1_66_0_nightly);
		assert!(version_1_65_0 > version_1_66_0_nightly_older_date);
		assert!(version_1_65_0 > version_1_65_0_older_date);

		let mut versions = vec![
			version_1_66_0,
			version_1_66_0_nightly,
			version_1_66_0_nightly_older_date,
			version_1_65_0_older_date,
			version_1_65_0,
			version_1_66_1,
		];
		versions.sort_by(|a, b| b.cmp(a));

		let expected_versions_order = vec![
			version_1_66_1,
			version_1_66_0,
			version_1_65_0,
			version_1_65_0_older_date,
			version_1_66_0_nightly,
			version_1_66_0_nightly_older_date,
		];
		assert_eq!(expected_versions_order, versions);
	}

	#[test]
	fn parse_with_newline() {
		let version_1_66_0 = Version::extract("cargo 1.66.0 (d65d197ad 2022-11-15)\n").unwrap();
		assert_eq!(
			Version {
				major: 1,
				minor: 66,
				patch: 0,
				is_nightly: false,
				year: 2022,
				month: 11,
				day: 15
			},
			version_1_66_0
		);
	}
}
