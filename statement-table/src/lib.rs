// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The statement table.
//!
//! This stores messages other authorities issue about candidates.
//!
//! These messages are used to create a proposal submitted to a BFT consensus process.
//!
//! Proposals are formed of sets of candidates which have the requisite number of
//! validity and availability votes.
//!
//! Each parachain is associated with two sets of authorities: those which can
//! propose and attest to validity of candidates, and those who can only attest
//! to availability.

pub mod generic;

pub use generic::Table;

use primitives::parachain::{
	Id, AbridgedCandidateReceipt, CompactStatement as PrimitiveStatement, ValidatorSignature, ValidatorIndex,
};
use primitives::Hash;

/// Statements about candidates on the network.
pub type Statement = generic::Statement<AbridgedCandidateReceipt, Hash>;

/// Signed statements about candidates.
pub type SignedStatement = generic::SignedStatement<
	AbridgedCandidateReceipt,
	Hash,
	ValidatorIndex,
	ValidatorSignature,
>;

/// Kinds of misbehavior, along with proof.
pub type Misbehavior = generic::Misbehavior<
	AbridgedCandidateReceipt,
	Hash,
	ValidatorIndex,
	ValidatorSignature,
>;

/// A summary of import of a statement.
pub type Summary = generic::Summary<Hash, Id>;

/// Context necessary to construct a table.
pub trait Context {
	/// Whether a authority is a member of a group.
	/// Members are meant to submit candidates and vote on validity.
	fn is_member_of(&self, authority: ValidatorIndex, group: &Id) -> bool;

	/// requisite number of votes for validity from a group.
	fn requisite_votes(&self, group: &Id) -> usize;
}

impl<C: Context> generic::Context for C {
	type AuthorityId = ValidatorIndex;
	type Digest = Hash;
	type GroupId = Id;
	type Signature = ValidatorSignature;
	type Candidate = AbridgedCandidateReceipt;

	fn candidate_digest(candidate: &AbridgedCandidateReceipt) -> Hash {
		candidate.hash()
	}

	fn candidate_group(candidate: &AbridgedCandidateReceipt) -> Id {
		candidate.parachain_index.clone()
	}

	fn is_member_of(&self, authority: &Self::AuthorityId, group: &Id) -> bool {
		Context::is_member_of(self, *authority, group)
	}

	fn requisite_votes(&self, group: &Id) -> usize {
		Context::requisite_votes(self, group)
	}
}

impl<'a> From<&'a Statement> for PrimitiveStatement {
	fn from(s: &'a Statement) -> PrimitiveStatement {
		match *s {
			generic::Statement::Valid(s) => PrimitiveStatement::Valid(s),
			generic::Statement::Invalid(s) => PrimitiveStatement::Invalid(s),
			generic::Statement::Candidate(ref s) => PrimitiveStatement::Candidate(s.hash()),
		}
	}
}
