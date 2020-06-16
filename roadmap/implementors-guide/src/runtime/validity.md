# Validity Module

After a backed candidate is made available, it is included and proceeds into an acceptance period during which validators are randomly selected to do (secondary) approval checks of the parablock. Any reports disputing the validity of the candidate will cause escalation, where even more validators are requested to check the block, and so on, until either the parablock is determined to be invalid or valid. Those on the wrong side of the dispute are slashed and, if the parablock is deemed invalid, the relay chain is rolled back to a point before that block was included.

However, this isn't the end of the story. We are working in a forkful blockchain environment, which carries three important considerations:

1. For security, validators that misbehave shouldn't only be slashed on one fork, but on all possible forks. Validators that misbehave shouldn't be able to create a new fork of the chain when caught and get away with their misbehavior.
1. It is possible that the parablock being contested has not appeared on all forks.
1. If a block author believes that there is a disputed parablock on a specific fork that will resolve to a reversion of the fork, that block author is better incentivized to build on a different fork which does not include that parablock.

This means that in all likelihood, there is the possibility of disputes that are started on one fork of the relay chain, and as soon as the dispute resolution process starts to indicate that the parablock is indeed invalid, that fork of the relay chain will be abandoned and the dispute will never be fully resolved on that chain.

Even if this doesn't happen, there is the possibility that there are two disputes underway, and one resolves leading to a reversion of the chain before the other has concluded. In this case we want to both transplant the concluded dispute onto other forks of the chain as well as the unconcluded dispute.

We account for these requirements by having the validity module handle two kinds of disputes.

1. Local disputes: those contesting the validity of the current fork by disputing a parablock included within it.
1. Remote disputes: a dispute that has partially or fully resolved on another fork which is transplanted to the local fork for completion and eventual slashing.

## Local Disputes

> TODO: store all included candidate and attestations on them here. accept additional backing after the fact. accept reports based on VRF. candidate included in session S should only be reported on by validator keys from session S. trigger slashing. probably only slash for session S even if the report was submitted in session S+k because it is hard to unify identity

One first question is to ask why different logic for local disputes is necessary. It seems that local disputes are necessary in order to create the first escalation that leads to block producers abandoning the chain and making remote disputes possible.

Local disputes are only allowed on parablocks that have been included on the local chain and are in the acceptance period.

For each such parablock, it is guaranteed by the inclusion pipeline that the parablock is available and the relevant validation code is available.

Disputes may occur against blocks that have happened in the session prior to the current one, from the perspective of the chain. In this case, the prior validator set is responsible for handling the dispute and to do so with their keys from the last session. This means that validator duty actually extends 1 session beyond leaving the validator set.

Validators self-select based on the BABE VRF output included by the block author in the block that the candidate became available.

> TODO: some more details from Jeff's paper.

After enough validators have self-selected, the quorum will be clear and validators on the wrong side will be slashed. After concluding, the dispute will remain open for some time in order to collect further evidence of misbehaving validators, and then issue a signal in the header-chain that this fork should be abandoned along with the hash of the last ancestor before inclusion, which the chain should be reverted to, along with information about the invalid block that should be used to blacklist it from being included.

## Remote Disputes

When a dispute has occurred on another fork, we need to transplant that dispute to every other fork. This poses some major challenges.

There are two types of remote disputes. The first is a remote roll-up of a concluded dispute. These are simply all attestations for the block, those against it, and the result of all (secondary) approval checks. A concluded remote dispute can be resolved in a single transaction as it is an open-and-shut case of a quorum of validators disagreeing with another.

The second type of remote dispute is the unconcluded dispute. An unconcluded remote dispute is started by any validator, using these things:

- A candidate
- The session that the candidate has appeared in.
- Backing for that candidate
- The validation code necessary for validation of the candidate.
  > TODO: optimize by excluding in case where code appears in `Paras::CurrentCode` of this fork of relay-chain
- Secondary checks already done on that candidate, containing one or more disputes by validators. None of the disputes are required to have appeared on other chains.
  > TODO: validator-dispute could be instead replaced by a fisherman w/ bond

When beginning a remote dispute, at least one escalation by a validator is required, but this validator may be malicious and desires to be slashed. There is no guarantee that the para is registered on this fork of the relay chain or that the para was considered available on any fork of the relay chain.

So the first step is to have the remote dispute proceed through an availability process similar to the one in the [Inclusion Module](inclusion.html), but without worrying about core assignments or compactness in bitfields.

We assume that remote disputes are with respect to the same validator set as on the current fork, as BABE and GRANDPA assure that forks are never long enough to diverge in validator set.
> TODO: this is at least directionally correct. handling disputes on other validator sets seems useless anyway as they wouldn't be bonded.

As with local disputes, the validators of the session the candidate was included on another chain are responsible for resolving the dispute and determining availability of the candidate.

If the candidate was not made available on another fork of the relay chain, the availability process will time out and the disputing validator will be slashed on this fork. The escalation used by the validator(s) can be replayed onto other forks to lead the wrongly-escalating validator(s) to be slashed on all other forks as well. We assume that the adversary cannot censor validators from seeing any particular forks indefinitely

> TODO: set the availability timeout for this accordingly - unlike in the inclusion pipeline we are slashing for unavailability here!

If the availability process passes, the remote dispute is ready to be included on this chain. As with the local dispute, validators self-select based on a VRF. Given that a remote dispute is likely to be replayed across multiple forks, it is important to choose a VRF in a way that all forks processing the remote dispute will have the same one. Choosing the VRF is important as it should not allow an adversary to have control over who will be selected as a secondary approval checker.

After enough validator self-select, under the same escalation rules as for local disputes, the Remote dispute will conclude, slashing all those on the wrong side of the dispute. After concluding, the remote dispute remains open for a set amount of blocks to accept any further proof of additional validators being on the wrong side.

## Slashing and Incentivization

The goal of the dispute is to garner a `>2/3` (`2f + 1`) supermajority either in favor of or against the candidate.

For remote disputes, it is possible that the parablock disputed has never actually passed any availability process on any chain. In this case, validators will not be able to obtain the PoV of the parablock and there will be relatively few votes. We want to disincentivize voters claiming validity of the block from preventing it from becoming available, so we charge them a small distraction fee for wasting the others' time if the dispute does not garner a 2/3+ supermajority on either side. This fee can take the form of a small slash or a reduction in rewards.

When a supermajority is achieved for the dispute in either the valid or invalid direction, we will penalize non-voters either by issuing a small slash or reducing their rewards. We prevent censorship of the remaining validators by leaving the dispute open for some blocks after resolution in order to accept late votes.
