# Architecture Decision Record

Architecture Decision Records are a lightweight way of keeping track of critical design decisions and the reasoning behind them.

For further information, please checkout <https://adr.github.io> or [https://heise.de (German)](https://www.heise.de/hintergrund/Gut-dokumentiert-Architecture-Decision-Records-4664988.html?seite=all).
.template
## Placement

It's recommended to create a folder `architecture` in the relevant crate (if the change is scoped to that trait) or the manifest root, where all project overarching ADRs are collected.

## Template

We are useing a derivation of a public [template by Michael Nygard](https://github.com/joelparkerhenderson/architecture-decision-record/blob/main/templates/decision-record-template-by-michael-nygard/index.md):

```md
# Title

## Status

<!-- What is the status, such as proposed, accepted, rejected, deprecated, superseded, etc.? -->

## Context

<!-- What is the issue that we're seeing that is motivating this decision or change? -->

## Decision

<!-- What is the change that we're proposing and/or doing? -->

## Consequences

<!-- What becomes easier or more difficult to do because of this change? -->
```
