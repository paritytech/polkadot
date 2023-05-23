// Copyright (C) Parity Technologies (UK) Ltd.
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

#[test]
fn alias_sibling_account_should_work() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<SiblingPrefix, IsNativeAccountId32>::contains(
		&(Parent, Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<SiblingPrefix, IsNativeAccountId32>::contains(
		&(Parent, Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}

#[test]
fn alias_child_account_should_work() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<ChildPrefix, IsNativeAccountId32>::contains(
		&(Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<ChildPrefix, IsNativeAccountId32>::contains(
		&(Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}

#[test]
fn alias_parent_account_should_work() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<ParentPrefix, IsNativeAccountId32>::contains(
		&(Parent, AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<ParentPrefix, IsNativeAccountId32>::contains(
		&(Parent, AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}
