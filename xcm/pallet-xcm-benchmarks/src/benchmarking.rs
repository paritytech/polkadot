// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg(feature = "runtime-benchmarks")]

use crate::*;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};

benchmarks! {
	withdraw_asset {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	reserve_asset_deposit {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	teleport_asset {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	query_response {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	transfer_asset {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	transfer_reserved_asset {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	transact {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	hrmp_channel_open_request {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	hrmp_channel_accepted {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	hrmp_channel_closing {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
	relayed_from {
		// setup
	}: {
		// execute
	} verify {
		// verify
	}
}

impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
