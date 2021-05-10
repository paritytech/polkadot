use proc_macro2::{Span};
use quote::quote;
use syn::Generics;
use syn::{Ident, Result};

use super::*;


pub(crate) fn impl_overseer_struct(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = &info.message_wrapper.clone();
	let overseer_name = info.overseer_name.clone();
	let field_name = &info.subsystem_names();

	let field_ty = &info.subsystem_generic_types();

	let baggage_name = &info.baggage_names();
	let baggage_ty = &info.baggage_types();

	let baggage_generic_ty = &info.baggage_generic_types();

	let generics = quote! {
		< Ctx, #( #baggage_generic_ty, )* #( #field_ty, )* >
	};

	let _where_clause = quote! {
		where
			Ctx: SubsystemContext,
			#( #field_ty : Subsystem<Ctx> )*
	};

	let mut ts = quote! {
		pub struct #overseer_name #generics {
			#(
				#field_name: #field_ty,
			)*

			#(
				#baggage_name: #baggage_ty,
			)*
		}

		impl #generics #overseer_name #generics {
			pub async fn stop(mut self) {
				#(
					let _ = self. #field_name .send_signal(OverseerSignal::Conclude).await;
				)*
				loop {
					select! {
						_ = self.running_subsystems.next() => {
							if self.running_subsystems.is_empty() {
								break;
							}
						},
						_ = stop_delay => break,
						complete => break,
					}
				}
			}
		}

		pub async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
			#(
				self. #field_name .send_signal(signal.clone()).await;
			)*
			let _ = signal;

			Ok(())
		}

		pub async fn route_message(&mut self, msg: #message_wrapper) -> SubsystemResult<()> {
			match msg {
				#(
					#field_ty (msg) => self. #field_name .send_message(msg).await?,
				)*
			}
			Ok(())
		}
	};

	ts.extend(impl_builder(info)?);
	Ok(ts)
}

/// Implement a builder pattern.
pub(crate) fn impl_builder(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let overseer_name = info.overseer_name.clone();
	let builder = Ident::new(&(overseer_name.to_string() + "Builder"), overseer_name.span());
	let handler = Ident::new(&(overseer_name.to_string() + "Handler"), overseer_name.span());

	let field_name = &info.subsystem_names();
	let field_ty = &info.subsystem_generic_types();

	let channel_name_unbounded = &info.channel_names("_unbounded");

	let channel_name_tx = &info.channel_names("_tx");
	let channel_name_unbounded_tx = &info.channel_names("_unbounded_tx");

	let channel_name_rx = &info.channel_names("_rx");
	let channel_name_unbounded_rx = &info.channel_names("_unbounded_rx");

	let baggage_generic_ty = &info.baggage_generic_types();
	let baggage_name = &info.baggage_names();

	let generics = quote! {
		< Ctx, #( #baggage_generic_ty, )* #( #field_ty, )* >
	};

	let where_clause = quote! {
		where
			#( #field_ty : Subsystem<Ctx>, )*
	};

	let ts = quote! {

		impl #generics #overseer_name #generics #where_clause {
			fn builder() -> #builder {
				#builder :: default()
			}
		}

		#[derive(Debug, Clone, Default)]
		struct #builder #generics {
			#(
				#field_name : ::std::option::Option< #field_ty >,
			)*
			#(
				#baggage_name : ::std::option::Option< #baggage_name >,
			)*
		}

		impl #generics #builder #generics #where_clause {
			#(
				fn #field_name (mut self, new: #field_ty ) -> #builder {
					self.#field_name = Some( new );
					self
				}
			)*

			fn build(mut self, ctx: Ctx) -> (#overseer_name #generics, #handler) {
				let overseer = #overseer_name :: #generics {
					#(
						#field_name : self. #field_name .unwrap(),
					)*
					#(
						#baggage_name : self. #baggage_name .unwrap(),
					)*
				};

				const CHANNEL_CAPACITY: usize = 1024;
				const SIGNAL_CHANNEL_CAPACITY: usize = 64;
				let (events_tx, events_rx) = ::metered::channel(SIGNAL_CHANNEL_CAPACITY);

				let handler = #handler {
					events_tx: events_tx.clone(),
				};


				let (to_overseer_tx, to_overseer_rx) = metered::unbounded();

				let mut running_subsystems = FuturesUnordered::new();


				let channels_out = {
					#(
						let (#channel_name_tx, #channel_name_rx) = ::metered::channel::<MessagePacket< #field_ty >>(CHANNEL_CAPACITY);
					)*

					#(
						let (#channel_name_unbounded_tx, #channel_name_unbounded_rx) = ::metered::unbounded::<MessagePacket< #field_ty >>();
					)*

					ChannelsOut {
						#(
							channel_name: #channel_name_tx .clone(),
						)*
						#(
							#channel_name_unbounded: #channel_name_unbounded_tx .clone(),
						)*
					}
				}


				// #( #launch subsystem )

				(overseer, handler)
			}
		}
	};
	Ok(ts)
}
