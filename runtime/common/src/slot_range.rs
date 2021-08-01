// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! The `SlotRange` struct which succinctly handles the 36 values that
//! represent all sub ranges between 0 and 7 inclusive.

slot_range_helper::generate_slot_range!(Zero(0), One(1), Two(2), Three(3), Four(4), Five(5), Six(6), Seven(7));

// Will generate:
// pub enum SlotRange {
// 	ZeroZero,		0
// 	ZeroOne,		1
// 	ZeroTwo,		2
// 	ZeroThree,		3
// 	ZeroFour,		4
// 	ZeroFive,		5
// 	ZeroSix,		6
// 	ZeroSeven,		7
// 	OneOne,			8
// 	OneTwo,			9
// 	OneThree,		10
// 	OneFour,		11
// 	OneFive,		12
// 	OneSix,			13
// 	OneSeven,		14
// 	TwoTwo,			15
// 	TwoThree,		16
// 	TwoFour,		17
// 	TwoFive,		18
// 	TwoSix,			19
// 	TwoSeven,		20
// 	ThreeThree,		21
// 	ThreeFour,		22
// 	ThreeFive,		23
// 	ThreeSix,		24
// 	ThreeSeven,		25
// 	FourFour,		26
// 	FourFive,		27
// 	FourSix,		28
// 	FourSeven,		29
// 	FiveFive,		30
// 	FiveSix,		31
// 	FiveSeven,		32
// 	SixSix,			33
// 	SixSeven,		34
// 	SevenSeven,		35
// }
