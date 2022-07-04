// Copyright (C) 2022 Parity Technologies (UK) Ltd.
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

mod kw {
	syn::custom_keyword!(event);
	syn::custom_keyword!(signal);
	syn::custom_keyword!(error);
	syn::custom_keyword!(outgoing);
	syn::custom_keyword!(gen);
	syn::custom_keyword!(signal_capacity);
	syn::custom_keyword!(message_capacity);
	syn::custom_keyword!(subsystem);
	syn::custom_keyword!(prefix);
}

mod parse_orchestra_attr;
mod parse_orchestra_struct;

mod parse_subsystem_attr;

#[cfg(test)]
mod tests;

pub(crate) use self::{parse_orchestra_attr::*, parse_orchestra_struct::*};

pub(crate) use self::parse_subsystem_attr::*;
