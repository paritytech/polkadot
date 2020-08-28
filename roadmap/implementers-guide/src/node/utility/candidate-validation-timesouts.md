# Polkadot AnV: dealing with timeouts

We present a proposal to deal with timeouts when executing parablocks in AnV, that only executes Wasmtime, never Wasmi. 

**Warning.** We need to assume that even though validators can have considerably different execution times for the same parablock, the *ratio* of execution times stays roughly constant; i.e. if validator $v$ executes some parablock $B$ twice as fast as validator $v'$, then $v$ would also be roughly twice as fast as $v'$ when executing any other parablock $B'$ in any other parachain. We also need to assume that, for a fixed parablock $B$, the *average* processing times among all validators evolves slowly (i.e. not all validators will update their machines at the same time).

## Definitions and intuition

Every validator $v$ keeps track of an estimate $r_v$ of the ratio between her own execution time and the claimed average execution time of all validators, if all validators executed the same parablock. So, $r_v>1$ means that validator $v$ is slower than average, and vice versa. This estimate is updated after each block.

When a validator executes a parablock $B$, it reports her execution time $t_v(B)$ in milliseconds along with her current estimated ratio $r_v$. From the reported pair $(t_v(B), r_v)$ we learn that $v$'s estimate of the *average execution time* $t_{average}(B)$ of this parablock is $t_v(B)/r_v$. A validator should accept parablock $B$ only if her estimate of $t_{average}(B)$ is less than a threshold. In case of conflict, we escalate and ask all validators to compute their own estimates of $t_{average}(B)$, and take the supermajority vote as the right value. We slash validators who failed to estimate $t_{average}(B)$ accurately.

There is a constant $\alpha$ (say, $\alpha=3$), and three time limits $t_{good}, t_{bad}=\alpha\cdot t_{good}, t_{ugly}=\alpha^2 \cdot t_{good}$, expressed in milliseconds. Parameters $\alpha$ and $t_{good}$ are decided by governance. These parameters guarantee that a validator won't get slashed as long as her estimate of $t_{average}(B)$ is within a multiplicative factor $\alpha$ from the true one.

## Protocol description

**Backers.** A backer $v$ records her execution time $t_v(B)$ on parablock $B$ and backs it if $t_v(B)/r_v < t_{good}$. It reports the pair $(t_v(B), r_v)$. 

**Verifiers.** A verifier $v$ records her execution time $t_v(B)$ on $B$.
1. If $t_v(B)/r_v<t_{ugly}$, she approves it and reports $(t_v(B), r_v)$.
2. If it takes longer than this, she aborts it and reports $(t_v(B), r_v)$, where in this case $t_v(B)=r_v \cdot t_{ugly}$ is the abort time.
3. If $B$ is invalid, she reports "invalid".

**Regular case.** Let $Ver(B)$ be the set of verifiers of parablock $B$. If all verifiers in $Ver(B)$ report case 1, $B$ is accepted. Let $t_{Ver(B)}$ be the median of the set $\{t_v(B)/r_v: \ v\in Ver(B)\}$. If $t_{Ver(B)}\leq t_{bad}$, backers are paid as usual, but if $t_{Ver(B)}> t_{bad}$ we don't pay the backers, as a punishment for backing slow blocks. We can consider additional or alternative light punishments in the latter case, such as issuing "admonishments" to the backers, which do nothing technically buy may affect their reputation negatively.

**Timeout case.** If one or more of the verifiers in $Ver(B)$ report case 2., we escalate and ask all other validators to execute $B$ and report their pair $(t_v(B), r_v)$. Backers and verifiers of $B$ cannot report again. We then consider the distribution $\{t_v(B)/r_v\}$ of reported estimates of the average execution time $t_{average}(B)$. See the figure below.

![](https://i.imgur.com/ul4gGw3.jpg)

a. Suppose the set $\{t_v(B)/r_v\}$ is sorted from low to high, let $n$ be the total number of validators in Polkadot, and let $t$ be the value at position $2n/3$ counting from the right. If $t \geq t_{bad}$, we reject parablock $B$ and slash 100% anyone whose reported estimate $t_v(B)/r_v$ is below $t/\alpha$. In particular, all backers will be slashed. The intuition here is that we're confident that $t_{average}(B)\geq t$, so it's fair to slash those whose estimate is more than a multiplicative factor $\alpha$ away from it.

b. Else, let $t'$ be the value in the sorted set $\{t_v(B)/r_v\}$ at position $2n/3$ counting from the left. If $t'< t_{bad}$, we accept parablock $B$ and slash 100% anyone whose reported estimate $t_v(B)/r_v$ is above $\alpha\cdot t'$. In particular, anyone who reported a timeout will be slashed. The intuition here is that we're confident that $t_{average}(B) \leq t'$, so as before it's fair to slash those with an estimate that's off by a multiplicative factor $\alpha$.

c. Finally, if neither of the two cases above is true, we reject the block but don't slash anyone. If this ever happens the system should raise a red flag and governance should jump in and analyze the situation. It can be that not enough validators answered to the escalation (if there are less than $2n/3$ reports in total, cases a. and b. will never hold). Or it can be that the distribution of estimates $\{t_v(B)/r_v\}$ is too spread out, in which case we should increase the value of parameter $\alpha$.

**Adversarial verifier's dilemma.** A nice consequence of this protocol is that an adversarial verifier $v$ faces a dilemma, if she wants a parablock $B$ to get accepted even though it times out. If $v$ reports an estimate $t_v(B)/r_v\geq t_{bad}$, and an escalation takes place, her report makes case a. more likely, and the parablock gets rejected. Conversely, if she reports an estimate $t_v(B)/r_v< t_{bad}$, she runs the risk that case a. occurs with a value $t=t_{ugly}$ (if a supermajority of validators agrees that the parablock times out), in which case she gets slashed 100%, as her estimate is more than $\alpha$ away from $t$. 

**Updating the estimates $r_v$.** After each parablock, if $v$ is a backer or a verifier of parablock $B$, she updates her estimate $r_v$ as

$$r_v\leftarrow (1-\varepsilon)r_v + \varepsilon\cdot \frac{t_v(B)}{median\{t_{v'}(B): \ v'\in Ver(B)\}},$$

where $\varepsilon$ is a small constant set by governance (say, $\varepsilon=0.05$), and we recall that $Ver(B)$ is the set of verifiers of parablock $B$. Notice that this is an exponential moving average over the series of observations $\frac{t_v(B)}{median\{t_{v'}(B): \ v'\in Ver(B)\}}$ for the different parablocks $B$ that validator $v$ is assigned to, and that for any single parablock $B$ the value $\frac{t_v(B)}{median\{t_{v'}(B): \ v'\in Ver(B)\}}$ constitutes the best estimate of $r_v$ available to $v$.

We remark that a verifier cannot lie "too much" about her estimate $t_v(B)/r_v$ of the average execution time of $B$, as she runs the risk of getting slashed. However, she can still lie "for free" about both her reported execution time $t_v(B)$ and her value of $r_v$ and scale both by an arbitrary factor, as long as it is the same factor for both. To limit this attack, we can also slash a validator whose reported estimate $r_v$ is too far from 1 (say, it should be between 0.1 and 10). Hence, validators would need to take preventive measures to keep their node speeds always close to the average.