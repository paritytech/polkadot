// Copyright 2015-2019 Parity Technologies (UK) Ltd.
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

extern crate substrate_trie;
extern crate substrate_primitives;
extern crate trie_db;

use std::{env, fs::File, io::{Error, Write}, path::Path};

use substrate_trie::NodeCodec;
use substrate_primitives::Blake2Hasher;

fn main() -> Result<(), Error> {
	let out_dir = env::var("OUT_DIR").unwrap();
	let dest_path = Path::new(&out_dir).join("consts.rs");
	let mut f = File::create(&dest_path).unwrap();

	let empty_root = <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node();
	let bytes = empty_root.as_bytes();

	f.write_all(format!("pub const EMPTY_TRIE_ROOT: [u8; 32] = {:?};", bytes).as_bytes())
}
