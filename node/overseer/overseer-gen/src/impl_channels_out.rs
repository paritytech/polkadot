use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use quote::ToTokens;
use syn::AttrStyle;
use syn::Generics;
use syn::Field;
use syn::FieldsNamed;
use syn::Variant;
use syn::{parse2, Attribute, Error, GenericParam, Ident, PathArguments, Result, Type, TypeParam, WhereClause};
use syn::spanned::Spanned;

use super::*;

/// Implement the helper type `ChannelsOut` and `MessagePacket<T>`.
pub(crate) fn impl_channels_out_struct(
    wrapper: &Ident,
	subsystems: &[SubSysField],
	baggage: &[BaggageField],
) -> Result<proc_macro2::TokenStream> {
    let mut channel_name = subsystems.iter().map(|ssf|
        ssf.name.clone());
    let mut channel_name_unbounded = subsystems.iter().map(|ssf|
            Ident::new(ssf.name.span(), ssf.name.to_string() + "_unbounded")
        );
	let mut field_ty = subsystems.iter().map(|ssf| ssf.ty.clone());
	let ts = quote! {
		#[derive(Debug)]
		struct MessagePacket<T> {
			signals_received: usize,
			message: T,
		}

		fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
			MessagePacket {
				signals_received,
				message,
			}
		}

		pub struct ChannelsOut {
			#(
				pub #channel_name: ::metered::MeteredSender<MessagePacket< #field_ty >>,
			)*
            #(
				pub #channel_name_unbounded: ::metered::UnboundedMeteredSender<MessagePacket< #field_ty >>,
			)*
		}

        impl ChannelsOut {
            async pub fn send_and_log_error(
                &mut self,
                signals_received: usize,
                message: #wrapper,
            ) {
                let res = match message {
                #(
                    #wrapper :: #field_ty (msg) => {
                        self. #channel_name .send(make_packet(signals_received, msg)).await
                    },
                )
                };

                if res.is_err() {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "Failed to send a message to another subsystem",
                    );
                }
            }


            pub fn send_unbounded_and_log_error(
                &self,
                signals_received: usize,
                message: AllMessages,
            ) {
                let res = match message {
                #(
                    #wrapper :: #field_ty (msg) => {
                        self. #channel_name_unbounded .send(
                            make_packet(signals_received, msg)
                        ).await
                    },
                )
                };

                if res.is_err() {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "Failed to send a message to another subsystem",
                    );
                }
            }
        }

	};
	Ok(ts)
}
