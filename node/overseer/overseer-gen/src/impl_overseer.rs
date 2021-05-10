use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use quote::ToTokens;
use syn::AttrStyle;
use syn::Field;
use syn::FieldsNamed;
use syn::Variant;
use syn::Generics;
use syn::{parse2, Attribute, Error, GenericParam, Ident, PathArguments, Result, Type, TypeParam, WhereClause};

use super::*;


pub(crate) fn impl_overseer_struct(
	overseer_name: &Ident,
	message_wrapper: &Ident,
	orig_generics: Generics,
	subsystems: &[SubSysField],
	baggage: &[BaggageField],
) -> Result<proc_macro2::TokenStream> {
	let mut field_name = &subsystems.iter().map(|ssf| ssf.name.clone()).collect::<Vec<_>>();

	let mut field_ty = &subsystems.iter().map(|ssf| ssf.generic.clone()).collect::<Vec<_>>();

	let mut baggage_name = &baggage.iter().map(|bf| bf.field_name.clone()).collect::<Vec<_>>();
	let mut baggage_ty = &baggage.iter().map(|bf| bf.field_ty.clone()).collect::<Vec<_>>();

	let mut baggage_generic_ty = &baggage.iter().filter(|bf| bf.generic).map(|bf| bf.field_ty.clone()).collect::<Vec<_>>();

	let generics = quote! {
		< Ctx, #( #baggage_generic_ty, )* #( #field_ty, )* >
	};

	let where_clause = quote! {
		where
			Ctx: SubsystemContext,
			#( #field_ty : Subsystem<Ctx> )*
	};

	let mut x = quote! {
		pub struct #overseer_name #generics {
			#(
				#field_name: #field_ty,
			)*

			#(
				#baggage_name: #baggage_ty,
			)*
		}

		impl #generics #overseer_name #generics {
			pub async fn stop(mut self) -> {
				#(
					let _ = self. #field_name .send_signal(OverseerSignal::Conclude).await
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

		async pub fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
			#(
				self. #field_name .send_signal(signal.clone()).await,
			)*
			let _ = signal;

			Ok(())
		}

		async pub fn route_message(&mut self, msg: #message_wrapper) -> SubsystemResult<()> {
			match msg {
				#(
					#field_ty (msg) => self. #field_name .send_message(msg).await?,
				)*
			}
			Ok(())
		}
	};

	x.extend(impl_builder(overseer_name, subsystems, baggage)?);

	Ok(x)
}

/// Implement a builder pattern.
pub(crate) fn impl_builder(
	name: &Ident,
	subsystems: &[SubSysField],
	baggage: &[BaggageField],
) -> Result<proc_macro2::TokenStream> {
	let builder = Ident::new((name.to_string() + "Builder").as_str(), Span::call_site());

	let overseer = name.clone();
	let handler = Ident::new(&(overseer.to_string() + "Handler"), overseer.span());

	let field_name = &subsystems.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
	let field_ty = &subsystems.iter().map(|x| x.generic.clone()).collect::<Vec<_>>();

    let channel_name = subsystems.iter().map(|ssf|
        ssf.name.clone());
    let channel_name_unbounded = subsystems.iter().map(|ssf|
		Ident::new(&(ssf.name.to_string() + "_unbounded"), ssf.name.span())
	).collect::<Vec<_>>();
	let channel_name_tx = &subsystems.iter().map(|ssf|
		Ident::new(&(ssf.name.to_string() + "_tx"), ssf.name.span())
	).collect::<Vec<_>>();
	let channel_name_unbounded_tx = &subsystems.iter().map(|ssf|
		Ident::new(&(ssf.name.to_string() + "_unbounded_tx"), ssf.name.span())
	).collect::<Vec<_>>();
	let channel_name_rx = &subsystems.iter().map(|ssf|
		Ident::new(&(ssf.name.to_string() + "_rx"), ssf.name.span())
	).collect::<Vec<_>>();
	let channel_name_unbounded_rx = &subsystems.iter().map(|ssf|
		Ident::new(&(ssf.name.to_string() + "_unbounded_rx"), ssf.name.span())
	).collect::<Vec<_>>();

	let baggage_generic_ty = &baggage.iter().filter(|b| b.generic).map(|b| b.field_ty.clone()).collect::<Vec<_>>();

	let baggage_name = &baggage.iter().map(|x| x.field_name.clone()).collect::<Vec<_>>();
	let baggage_ty = &baggage.iter().map(|x| x.field_ty.clone()).collect::<Vec<_>>();

	let generics = quote! {
		< Ctx, #( #baggage_generic_ty, )* #( #field_ty, )* >
	};

	let where_clause = quote! {
		where
			#( #field_ty : Subsystem<Ctx>, )*
	};

	let ts = quote! {

		impl #generics #name #generics #where_clause {
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

			fn build(mut self, ctx: Ctx) -> (#overseer #generics, #handler) {
				let overseer = #overseer :: #generics {
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


				let metrics = <Metrics as metrics::Metrics>::register(prometheus_registry)?;

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
