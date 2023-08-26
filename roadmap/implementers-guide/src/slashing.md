# Slashing

Parachain consensus has two types of slashing:

  * Validators who back an invalid candidate (as decided by a ⅔ supermajority) are slashed **100%** of their bond. This is based on the principle of gambler's ruin, that actions that could result in consensus around an invalid state of the system must be expensive enough that they will quickly result in a validator losing their entire bond and being removed from the validator set.
  
  * Validators who dispute a valid candidate are collectively slashed **1%** of the minimum of their bond, divided evenly between them. This is essentially spam prevention as it attaches a cost to issuing erroneous disputes that put load on the network by forcing all validators to check a candidate. In special cases where the candidate takes many approval checkers a long time to execute, the 1% slash is also split with the backer who is charged for the time overrun.

# Time Overruns

One reason candidate validation can fail is if PVF execution reaches a timeout. This timeout is set to two seconds for backers and 84 seconds for approval voters, assuming 89 delay tranches and one second tick size. Discrepancies in execution time between validators can be due to resource differences or nondeterminism, both of which can be mitigated by using deterministic wasm with instruction metering, but also by cache misses even with metered wasm. In practice we see about a 300% variance in execution due to these factors. Beyond that, we should suspect malice on the part of the backer and/or collator either in crafting malicious state transitions or not having executed the candidate themselves prior to issuing a backing statement.

If a supermajority of approval checkers reach the execution timeout, the candidate is considered invalid and therefore only the backer is slashed. However, if less than one third of approval checkers time out,  they could be slashed for voting against a valid candidate despite being honest. We handle this by charging backers for lengthy executions and subtracting this charge from slashes against approval checkers who dispute valid candidates.

Additionally, discouraging lengthy executions allows us to raise the execution timeout to much longer than the no-show timeout (twelve seconds) after which we assume validators could be offline due to a denial of service attack. No-shows are preferable to disputes in terms of additional network load, but still should be avoided. With time overruns we can use an approval checker execution timeout close to the point where all delay tranches would have been reached (remembering that the first few tranches could possibly be called on without no-shows if there aren't enough validators in the zeroth tranche to provide the needed approvals).

In order to detect lengthy execution, approval checkers include how long it took them to execute it with their vote. When the median execution time is suspiciously long, the backer is charged proportional to the median time. Time overruns can occur regardless of whether a dispute is issued, but in the case of disputes some edge cases around them occur.

## Calculating Overrun Charges

Backers begin being charged for an overrun when the median execution time is over six seconds (three times the backing timeout). The charge increases exponentially until 28 seconds, or one third of the approval checking timeout, at which point the backer is charged 1% of their bond.

## Disputes Concluding Invalid

When a supermajority of validators conclude a candidate is invalid, the backer is already slashed 100% so cannot be charged additionally for a time overrun.

## Disputes Concluded Valid

When a supermajority of validators conclude a disputed candidate is valid, the validators who voted against it are collectively slashed 1% of the minimum of their bond minus any time overrun charge. For example, if two validators dispute a valid candidate and they have 500,000 and 550,000 DOT in bond respectively, then they're each slashed 5,000 DOT assuming no time overrun. If however, the backer is charged 2,500 DOT for a time overrun, then each disputing validator would be slashed 1,250 DOT.

## Malicious Time Overruns

One significant problem with using median execution time for time overruns is that we cannot assume a 51% honest majority of approval checkers when there isn't a dispute, that is when the entire validator set isn't voting. While Polkadot assumes an honest ⅔ supermajority of validators (that is, it is practically byzantine fault tolerant) approval checking only relies on it being rare enough that 100% of validators who end up checking a candidate will be dishonest. Assuming a maximum 1/3 malicious validators and average of 40 approval checkers, there's a 2.14% chance over 50% of approval checkers are malicious and could collude to inflate their time reports in order to charge the backer for an overrun when none occurred. 

We handle this case when the median time report could be controlled by dishonest validators by allowing validators, including the backer themselves, to dispute a candidate while adding a flag that they consider the candidate to be valid and quoting time reports they consider suspicious. This calls on the entire validator set to execute the candidate, which gives us an honest median time report. If half the needed approvals have time reports greater than three times the median after the dispute, those approval checkers are slashed as if they had issued an erroneous dispute. If this is not the case, that is the dispute does not provide evidence that a dangerous number of approval checkers were inflating their time reports, then the validator who raised this dispute is slashed 1%.
