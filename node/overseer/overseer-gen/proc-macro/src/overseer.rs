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

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use syn::{parse2, parse_quote, punctuated::Punctuated, spanned::Spanned, Result};

use super::{parse::*, *};

pub(crate) fn impl_overseer_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let args: OverseerAttrArgs = parse2(attr)?;
	let message_wrapper = args.message_wrapper;

	let of: OverseerGuts = parse2(orig)?;

	let support_crate = support_crate();
	let info = OverseerInfo {
		support_crate,
		subsystems: of.subsystems,
		baggage: of.baggage,
		overseer_name: of.name,
		message_wrapper,
		message_channel_capacity: args.message_channel_capacity,
		signal_channel_capacity: args.signal_channel_capacity,
		extern_event_ty: args.extern_event_ty,
		extern_signal_ty: args.extern_signal_ty,
		extern_error_ty: args.extern_error_ty,
		outgoing_ty: args.outgoing_ty,
	};

	let mut additive = impl_overseer_struct(&info);
	additive.extend(impl_builder(&info));

	additive.extend(impl_overseen_subsystem(&info));
	additive.extend(impl_channels_out_struct(&info));
	additive.extend(impl_subsystem(&info)?);

	additive.extend(impl_message_wrapper_enum(&info)?);

	let ts = expander::Expander::new("overlord-expansion")
		.add_comment("Generated overseer code by `#[overlord(..)]`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(true)
		.fmt(expander::Edition::_2021)
		.write_to_out_dir(additive)
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
}
