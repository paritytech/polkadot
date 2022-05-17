// Copyright 2022 Parity Technologies (UK) Ltd.
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

use proc_macro2::TokenStream;
use syn::{parse2, Result};

use super::{parse::*, *};

pub(crate) fn impl_orchestra_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let args: OrchestraAttrArgs = parse2(attr)?;
	let message_wrapper = args.message_wrapper;

	let of: OrchestraGuts = parse2(orig)?;

	let support_crate = support_crate().expect("The crate this macro is run for, includes the proc-macro support as dependency, otherwise it could not be run in the first place. qed");
	let info = OrchestraInfo {
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

	let mut additive = impl_orchestra_struct(&info);
	additive.extend(impl_builder(&info));

	additive.extend(impl_orchestrated_subsystem(&info));
	additive.extend(impl_channels_out_struct(&info));
	additive.extend(impl_subsystem_types_all(&info)?);

	additive.extend(impl_message_wrapper_enum(&info)?);

	let ts = expander::Expander::new("orchestra-expansion")
		.add_comment("Generated overseer code by `#[orchestra(..)]`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(true)
		// once all our needed format options are available on stable
		// we should enabled this again, until then too many warnings
		// are generated
		// .fmt(expander::Edition::_2021)
		.write_to_out_dir(additive)
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
}
