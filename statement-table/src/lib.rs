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

pub use generic::{Table, Context};

/// Concrete instantiations suitable for v0 primitives.
pub mod v0 {
	use crate::generic;
	use primitives::v0::{
		Hash,
		Id, AbridgedCandidateReceipt, CompactStatement as PrimitiveStatement, ValidatorSignature, ValidatorIndex,
	};

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

	impl<'a> From<&'a Statement> for PrimitiveStatement {
		fn from(s: &'a Statement) -> PrimitiveStatement {
			match *s {
				generic::Statement::Valid(s) => PrimitiveStatement::Valid(s),
				generic::Statement::Invalid(s) => PrimitiveStatement::Invalid(s),
				generic::Statement::Candidate(ref s) => PrimitiveStatement::Candidate(s.hash()),
			}
		}
	}
}

/// Concrete instantiations suitable for v1 primitives.
pub mod v1 {
	use crate::generic;
	use primitives::v1::{
		Hash,
		Id, CommittedCandidateReceipt, CompactStatement as PrimitiveStatement,
		ValidatorSignature, ValidatorIndex,
	};

	/// Statements about candidates on the network.
	pub type Statement = generic::Statement<CommittedCandidateReceipt, Hash>;

	/// Signed statements about candidates.
	pub type SignedStatement = generic::SignedStatement<
		CommittedCandidateReceipt,
		Hash,
		ValidatorIndex,
		ValidatorSignature,
	>;

	/// Kinds of misbehavior, along with proof.
	pub type Misbehavior = generic::Misbehavior<
		CommittedCandidateReceipt,
		Hash,
		ValidatorIndex,
		ValidatorSignature,
	>;

	/// A summary of import of a statement.
	pub type Summary = generic::Summary<Hash, Id>;

	impl<'a> From<&'a Statement> for PrimitiveStatement {
		fn from(s: &'a Statement) -> PrimitiveStatement {
			match *s {
				generic::Statement::Valid(s) => PrimitiveStatement::Valid(s),
				generic::Statement::Invalid(s) => PrimitiveStatement::Invalid(s),
				generic::Statement::Candidate(ref s) => PrimitiveStatement::Candidate(s.hash()),
			}
		}
	}
}
