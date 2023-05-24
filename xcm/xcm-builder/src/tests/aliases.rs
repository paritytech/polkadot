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
fn alias_foreign_account_sibling_prefix() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<SiblingPrefix>::contains(
		&(Parent, Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<SiblingPrefix>::contains(
		&(Parent, Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}

#[test]
fn alias_foreign_account_child_prefix() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<ChildPrefix>::contains(
		&(Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<ChildPrefix>::contains(
		&(Parachain(1), AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}

#[test]
fn alias_foreign_account_parent_prefix() {
	// Accounts Differ
	assert!(!AliasForeignAccountId32::<ParentPrefix>::contains(
		&(Parent, AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [1; 32] }).into()
	));

	assert!(AliasForeignAccountId32::<ParentPrefix>::contains(
		&(Parent, AccountId32 { network: None, id: [0; 32] }).into(),
		&(AccountId32 { network: None, id: [0; 32] }).into()
	));
}

#[test]
fn alias_case_should_work() {
	// Wrong Origin
	assert!(!AliasCase::<SpecificParachain, SpecificPlurality>::contains(
		&(Parent, Parachain(2)).into(),
		&(Plurality { id: BodyId::Treasury, part: BodyPart::Voice }).into()
	));

	// Wrong BodyId
	assert!(!AliasCase::<SpecificParachain, SpecificPlurality>::contains(
		&(Parent, Parachain(1)).into(),
		&(Plurality { id: BodyId::Administration, part: BodyPart::Voice }).into()
	));

	assert!(AliasCase::<SpecificParachain, SpecificPlurality>::contains(
		&(Parent, Parachain(1)).into(),
		&(Plurality { id: BodyId::Treasury, part: BodyPart::Voice }).into()
	));
}
