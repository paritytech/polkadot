// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.
//

//! Helpers related to signatures.
//!
//! Used for testing and benchmarking.

// reexport to avoid direct secp256k1 deps by other crates
pub use secp256k1::SecretKey;

use crate::{
	public_to_address, rlp_encode, step_validator, Address, AuraHeader, RawTransaction, UnsignedTransaction, H256,
	H520, U256,
};

use secp256k1::{Message, PublicKey};

/// Utilities for signing headers.
pub trait SignHeader {
	/// Signs header by given author.
	fn sign_by(self, author: &SecretKey) -> AuraHeader;
	/// Signs header by given authors set.
	fn sign_by_set(self, authors: &[SecretKey]) -> AuraHeader;
}

/// Utilities for signing transactions.
pub trait SignTransaction {
	/// Sign transaction by given author.
	fn sign_by(self, author: &SecretKey, chain_id: Option<u64>) -> RawTransaction;
}

impl SignHeader for AuraHeader {
	fn sign_by(mut self, author: &SecretKey) -> Self {
		self.author = secret_to_address(author);

		let message = self.seal_hash(false).unwrap();
		let signature = sign(author, message);
		self.seal[1] = rlp_encode(&signature).to_vec();
		self
	}

	fn sign_by_set(self, authors: &[SecretKey]) -> Self {
		let step = self.step().unwrap();
		let author = step_validator(authors, step);
		self.sign_by(author)
	}
}

impl SignTransaction for UnsignedTransaction {
	fn sign_by(self, author: &SecretKey, chain_id: Option<u64>) -> RawTransaction {
		let message = self.message(chain_id);
		let signature = sign(author, message);
		let signature_r = U256::from_big_endian(&signature.as_fixed_bytes()[..32][..]);
		let signature_s = U256::from_big_endian(&signature.as_fixed_bytes()[32..64][..]);
		let signature_v = signature.as_fixed_bytes()[64] as u64;
		let signature_v = signature_v + if let Some(n) = chain_id { 35 + n * 2 } else { 27 };

		let mut stream = rlp::RlpStream::new_list(9);
		self.rlp_to(None, &mut stream);
		stream.append(&signature_v);
		stream.append(&signature_r);
		stream.append(&signature_s);
		stream.out().to_vec()
	}
}

/// Return author's signature over given message.
pub fn sign(author: &SecretKey, message: H256) -> H520 {
	let (signature, recovery_id) = secp256k1::sign(&Message::parse(message.as_fixed_bytes()), author);
	let mut raw_signature = [0u8; 65];
	raw_signature[..64].copy_from_slice(&signature.serialize());
	raw_signature[64] = recovery_id.serialize();
	raw_signature.into()
}

/// Returns address corresponding to given secret key.
pub fn secret_to_address(secret: &SecretKey) -> Address {
	let public = PublicKey::from_secret_key(secret);
	let mut raw_public = [0u8; 64];
	raw_public.copy_from_slice(&public.serialize()[1..]);
	public_to_address(&raw_public)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{transaction_decode_rlp, Transaction};

	#[test]
	fn transaction_signed_properly() {
		// case1: with chain_id replay protection + to
		let signer = SecretKey::parse(&[1u8; 32]).unwrap();
		let signer_address = secret_to_address(&signer);
		let unsigned = UnsignedTransaction {
			nonce: 100.into(),
			gas_price: 200.into(),
			gas: 300.into(),
			to: Some([42u8; 20].into()),
			value: 400.into(),
			payload: vec![1, 2, 3],
		};
		let raw_tx = unsigned.clone().sign_by(&signer, Some(42));
		assert_eq!(
			transaction_decode_rlp(&raw_tx),
			Ok(Transaction {
				sender: signer_address,
				unsigned,
			}),
		);

		// case2: without chain_id replay protection + contract creation
		let unsigned = UnsignedTransaction {
			nonce: 100.into(),
			gas_price: 200.into(),
			gas: 300.into(),
			to: None,
			value: 400.into(),
			payload: vec![1, 2, 3],
		};
		let raw_tx = unsigned.clone().sign_by(&signer, None);
		assert_eq!(
			transaction_decode_rlp(&raw_tx),
			Ok(Transaction {
				sender: signer_address,
				unsigned,
			}),
		);
	}
}
