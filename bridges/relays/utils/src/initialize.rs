// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

use std::{cell::RefCell, fmt::Display, io::Write};

async_std::task_local! {
	pub(crate) static LOOP_NAME: RefCell<String> = RefCell::new(String::default());
}

/// Initialize relay environment.
pub fn initialize_relay() {
	initialize_logger(true);
}

/// Initialize Relay logger instance.
pub fn initialize_logger(with_timestamp: bool) {
	let mut builder = env_logger::Builder::new();
	builder.filter_level(log::LevelFilter::Warn);
	builder.filter_module("bridge", log::LevelFilter::Info);
	builder.parse_default_env();
	if with_timestamp {
		builder.format(move |buf, record| {
			let timestamp = time::OffsetDateTime::try_now_local()
				.unwrap_or_else(|_| time::OffsetDateTime::now_utc())
				.format("%Y-%m-%d %H:%M:%S %z");

			let log_level = color_level(record.level());
			let log_target = color_target(record.target());
			let timestamp = if cfg!(windows) {
				Either::Left(timestamp)
			} else {
				Either::Right(ansi_term::Colour::Fixed(8).bold().paint(timestamp))
			};

			writeln!(
				buf,
				"{}{} {} {} {}",
				loop_name_prefix(),
				timestamp,
				log_level,
				log_target,
				record.args(),
			)
		});
	} else {
		builder.format(move |buf, record| {
			let log_level = color_level(record.level());
			let log_target = color_target(record.target());

			writeln!(
				buf,
				"{}{} {} {}",
				loop_name_prefix(),
				log_level,
				log_target,
				record.args(),
			)
		});
	}

	builder.init();
}

/// Initialize relay loop. Must only be called once per every loop task.
pub(crate) fn initialize_loop(loop_name: String) {
	LOOP_NAME.with(|g_loop_name| *g_loop_name.borrow_mut() = loop_name);
}

/// Returns loop name prefix to use in logs. The prefix is initialized with the `initialize_loop` call.
fn loop_name_prefix() -> String {
	// try_with to avoid panic outside of async-std task context
	LOOP_NAME
		.try_with(|loop_name| {
			// using borrow is ok here, because loop is only initialized once (=> borrow_mut will only be called once)
			let loop_name = loop_name.borrow();
			if loop_name.is_empty() {
				String::new()
			} else {
				format!("[{}] ", loop_name)
			}
		})
		.unwrap_or_else(|_| String::new())
}

enum Either<A, B> {
	Left(A),
	Right(B),
}
impl<A: Display, B: Display> Display for Either<A, B> {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		match self {
			Self::Left(a) => write!(fmt, "{}", a),
			Self::Right(b) => write!(fmt, "{}", b),
		}
	}
}

fn color_target(target: &str) -> impl Display + '_ {
	if cfg!(windows) {
		Either::Left(target)
	} else {
		Either::Right(ansi_term::Colour::Fixed(8).paint(target))
	}
}

fn color_level(level: log::Level) -> impl Display {
	if cfg!(windows) {
		Either::Left(level)
	} else {
		let s = level.to_string();
		use ansi_term::Colour as Color;
		Either::Right(match level {
			log::Level::Error => Color::Fixed(9).bold().paint(s),
			log::Level::Warn => Color::Fixed(11).bold().paint(s),
			log::Level::Info => Color::Fixed(10).paint(s),
			log::Level::Debug => Color::Fixed(14).paint(s),
			log::Level::Trace => Color::Fixed(12).paint(s),
		})
	}
}
