// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[test]
#[rustversion::attr(not(stable), ignore)]
fn ui_compile_fail() {
	// Only run the ui tests when `RUN_UI_TESTS` is set.
	if std::env::var("RUN_UI_TESTS").is_err() {
		return
	}

	let t = trybuild::TestCases::new();
	t.compile_fail("tests/ui/err-*.rs");
}

#[test]
#[rustversion::attr(not(stable), ignore)]
fn ui_pass() {
	// Only run the ui tests when `RUN_UI_TESTS` is set.
	if std::env::var("RUN_UI_TESTS").is_err() {
		return
	}

	let t = trybuild::TestCases::new();
	t.pass("tests/ui/ok-*.rs");
}
