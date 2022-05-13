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
		error=OverseerError,
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

	let output = impl_overseer_gen(attr, item).expect("Simple example always works. qed");
	println!("//generated:");
	println!("{}", output);
}

#[test]
fn struct_parse_full() {
	let item: OverseerGuts = parse_quote! {
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
	let item: OverseerGuts = parse_quote! {
		pub struct Ooooh {
			#[subsystem(Foo)]
			sub0: FooSubsystem,
		}
	};
	let _ = dbg!(item);
}

#[test]
fn attr_full() {
	let attr: OverseerAttrArgs = parse_quote! {
		gen=AllMessage, event=::some::why::ExternEvent, signal=SigSigSig, signal_capacity=111, message_capacity=222,
		error=OverseerError,
	};
	assert_matches!(attr, OverseerAttrArgs {
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
	let attr: OverseerAttrArgs = parse_quote! {
		gen=AllMessage, event=::some::why::ExternEvent, signal=::foo::SigSigSig,
		error=OverseerError,
	};
	assert_matches!(attr, OverseerAttrArgs {
		message_channel_capacity: _,
		signal_channel_capacity: _,
		..
	} => {
	});
}
