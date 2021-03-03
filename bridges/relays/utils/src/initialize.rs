// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Relayer initialization functions.

use std::io::Write;

/// Initialize relay environment.
pub fn initialize_relay() {
	let mut builder = env_logger::Builder::new();

	let filters = match std::env::var("RUST_LOG") {
		Ok(env_filters) => format!("bridge=info,{}", env_filters),
		Err(_) => "bridge=info".into(),
	};

	builder.parse_filters(&filters);
	builder.format(move |buf, record| {
		writeln!(buf, "{}", {
			let timestamp = time::OffsetDateTime::try_now_local()
				.unwrap_or_else(|_| time::OffsetDateTime::now_utc())
				.format("%Y-%m-%d %H:%M:%S %z");
			if cfg!(windows) {
				format!("{} {} {} {}", timestamp, record.level(), record.target(), record.args())
			} else {
				use ansi_term::Colour as Color;
				let log_level = match record.level() {
					log::Level::Error => Color::Fixed(9).bold().paint(record.level().to_string()),
					log::Level::Warn => Color::Fixed(11).bold().paint(record.level().to_string()),
					log::Level::Info => Color::Fixed(10).paint(record.level().to_string()),
					log::Level::Debug => Color::Fixed(14).paint(record.level().to_string()),
					log::Level::Trace => Color::Fixed(12).paint(record.level().to_string()),
				};
				format!(
					"{} {} {} {}",
					Color::Fixed(8).bold().paint(timestamp),
					log_level,
					Color::Fixed(8).paint(record.target()),
					record.args()
				)
			}
		})
	});

	builder.init();
}
