// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Client side declaration and registration of the parachain Prometheus metrics.
//! All of the metrics have a correspondent runtime metric definition.

use crate::runtime::RuntimeMetricsProvider;

/// Register the parachain runtime metrics.
pub fn register_metrics(runtime_metrics_provider: &RuntimeMetricsProvider) {
    runtime_metrics_provider.register_countervec(
        "parachain_inherent_data_weight", 
        "Inherent data weight before and after filtering",
        &["when"]
    );

    runtime_metrics_provider.register_counter(
		"parachain_inherent_data_bitfields_processed",
		"Counts the number of bitfields processed in `enter_inner`.",
	);

    runtime_metrics_provider.register_countervec(
		"parachain_inherent_data_candidates_processed",
		"Counts the number of parachain block candidates processed in `enter_inner`.",
		&["category"],
	);

    runtime_metrics_provider.register_countervec(
		"parachain_inherent_data_dispute_sets_processed",
		"Counts the number of dispute statements sets processed in `enter_inner`.",
		&["category"],
	);

    runtime_metrics_provider.register_counter(
		"parachain_inherent_data_dispute_sets_included",
		"Counts the number of dispute statements sets included in a block in `enter_inner`.",
	);

    runtime_metrics_provider.register_countervec(
		"parachain_create_inherent_bitfields_signature_checks",
		"Counts the number of bitfields signature checked in `enter_inner`.",
		&["validity"],
	)
}
