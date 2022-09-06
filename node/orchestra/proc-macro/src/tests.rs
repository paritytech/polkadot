// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

use super::*;
use assert_matches::assert_matches;
use quote::quote;
use syn::parse_quote;

#[test]
fn print() {
	let attr = quote! {
		gen=AllMessage,
		event=::some::why::ExternEvent,
		signal=SigSigSig,
		signal_capacity=111,
		message_capacity=222,
		error=OrchestraError,
	};

	let item = quote! {
		pub struct Ooooh<X = Pffffffft> where X: Secrit {
			#[subsystem(Foo)]
			sub0: FooSubsystem,

			#[subsystem(blocking, Bar)]
			yyy: BaersBuyBilliardBalls,

			#[subsystem(blocking, Twain)]
			fff: Beeeeep,

			#[subsystem(Rope)]
			mc: MountainCave,

			metrics: Metrics,
		}
	};

	let output = impl_orchestra_gen(attr, item).expect("Simple example always works. qed");
	println!("//generated:");
	println!("{}", output);
}

#[test]
fn struct_parse_full() {
	let item: OrchestraGuts = parse_quote! {
		pub struct Ooooh<X = Pffffffft> where X: Secrit {
			#[subsystem(Foo)]
			sub0: FooSubsystem,

			#[subsystem(blocking, Bar)]
			yyy: BaersBuyBilliardBalls,

			#[subsystem(blocking, Twain)]
			fff: Beeeeep,

			#[subsystem(Rope)]
			mc: MountainCave,

			metrics: Metrics,
		}
	};
	let _ = dbg!(item);
}

#[test]
fn struct_parse_basic() {
	let item: OrchestraGuts = parse_quote! {
		pub struct Ooooh {
			#[subsystem(Foo)]
			sub0: FooSubsystem,
		}
	};
	let _ = dbg!(item);
}

#[test]
fn attr_full() {
	let attr: OrchestraAttrArgs = parse_quote! {
		gen=AllMessage, event=::some::why::ExternEvent, signal=SigSigSig, signal_capacity=111, message_capacity=222,
		error=OrchestraError,
	};
	assert_matches!(attr, OrchestraAttrArgs {
		message_channel_capacity,
		signal_channel_capacity,
		..
	} => {
		assert_eq!(message_channel_capacity, 222);
		assert_eq!(signal_channel_capacity, 111);
	});
}

#[test]
fn attr_partial() {
	let attr: OrchestraAttrArgs = parse_quote! {
		gen=AllMessage, event=::some::why::ExternEvent, signal=::foo::SigSigSig,
		error=OrchestraError,
	};
	assert_matches!(attr, OrchestraAttrArgs {
		message_channel_capacity: _,
		signal_channel_capacity: _,
		..
	} => {
	});
}
