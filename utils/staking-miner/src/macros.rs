// NOTE: this is no longer used extensively, most of the per-runtime stuff us delegated to
// `construct_runtime_prelude` and macro's the import directly from it. A part of the code is also
// still generic over `T`. My hope is to still make everything generic over a `Runtime`, but sadly
// that is not currently possible as each runtime has its unique `Call`, and all Calls are not
// sharing any generic trait. In other words, to create the `UncheckedExtrinsic` of each chain, you
// need the concrete `Call` of that chain as well.
#[macro_export]
macro_rules! any_runtime {
	($($code:tt)*) => {
		unsafe {
			match $crate::RUNTIME {
				$crate::AnyRuntime::Polkadot => {
					#[allow(unused)]
					use $crate::polkadot_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Kusama => {
					#[allow(unused)]
					use $crate::kusama_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Westend => {
					#[allow(unused)]
					use $crate::westend_runtime_exports::*;
					$($code)*
				}
			}
		}
	}
}

/// Same as [`any_runtime`], but instead of returning a `Result`, this simply returns `()`. Useful
/// for situations where the result is not useful and un-ergonomic to handle.
#[macro_export]
macro_rules! any_runtime_unit {
	($($code:tt)*) => {
		unsafe {
			match $crate::RUNTIME {
				$crate::AnyRuntime::Polkadot => {
					#[allow(unused)]
					use $crate::polkadot_runtime_exports::*;
					let _ = $($code)*;
				},
				$crate::AnyRuntime::Kusama => {
					#[allow(unused)]
					use $crate::kusama_runtime_exports::*;
					let _ = $($code)*;
				},
				$crate::AnyRuntime::Westend => {
					#[allow(unused)]
					use $crate::westend_runtime_exports::*;
					let _ = $($code)*;
				}
			}
		}
	}
}

macro_rules! construct_runtime_prelude {
	($runtime:ident) => { paste::paste! {
		#[allow(unused_import)]
		pub(crate) mod [<$runtime _runtime_exports>] {
			pub(crate) use crate::prelude::EPM;
			pub(crate) use [<$runtime _runtime>]::*;
			pub(crate) use crate::monitor::[<monitor_cmd_ $runtime>] as monitor_cmd;
			pub(crate) use crate::dry_run::[<dry_run_cmd_ $runtime>] as dry_run_cmd;
			pub(crate) use crate::emergency_solution::[<emergency_solution_cmd_ $runtime>] as emergency_solution_cmd;
			pub(crate) use private::{[<create_uxt_ $runtime>] as create_uxt};

			mod private {
				use super::*;
				pub(crate) fn [<create_uxt_ $runtime>](
					raw_solution: EPM::RawSolution<EPM::SolutionOf<Runtime>>,
					signer: crate::signer::Signer,
					nonce: crate::prelude::Index,
					tip: crate::prelude::Balance,
					era: sp_runtime::generic::Era,
				) -> UncheckedExtrinsic {
					use codec::Encode as _;
					use sp_core::Pair as _;
					use sp_runtime::traits::StaticLookup as _;

					let crate::signer::Signer { account, pair, .. } = signer;

					let local_call = EPMCall::<Runtime>::submit { raw_solution: Box::new(raw_solution) };
					let call: Call = <EPMCall<Runtime> as std::convert::TryInto<Call>>::try_into(local_call)
						.expect("election provider pallet must exist in the runtime, thus \
							inner call can be converted, qed."
						);

					let extra: SignedExtra = crate::[<signed_ext_builder_ $runtime>](nonce, tip, era);
					let raw_payload = SignedPayload::new(call, extra).expect("creating signed payload infallible; qed.");
					let signature = raw_payload.using_encoded(|payload| {
						pair.sign(payload)
					});
					let (call, extra, _) = raw_payload.deconstruct();
					let address = <Runtime as frame_system::Config>::Lookup::unlookup(account);
					let extrinsic = UncheckedExtrinsic::new_signed(call, address, signature.into(), extra);
					log::debug!(
						target: crate::LOG_TARGET, "constructed extrinsic {} with length {}",
						sp_core::hexdisplay::HexDisplay::from(&extrinsic.encode()),
						extrinsic.encode().len(),
					);
					extrinsic
				}
			}
		}}
	};
}
