# Description

_Describe a summary of your changes clearly and concisely, including motivation and context._
_Breaking changes need extra explanation on backward compatibility considerations._

_Feel free to include additional details, but please respect the reviewer's time and keep it brief._


## Related issues

_Related issues can be listed here._

_(Remove the section if not applicable.)_


# Release notes

_If this is a user-facing change that should be mentioned in the release notes, please provide a draft of the notes here._

_(Remove the section if no notes are needed.)_


# Manual testing

_Please describe the tests that you ran to verify your changes._

_Provide clear instructions so the reviewers can reproduce and verify your results._

_Include relevant details for your test configuration, operating system, etc._

The change has been manually tested on:

- [ ] Linux
- [ ] macOS
- [ ] Windows


# Checklist

_Please tick the items as you ave addressed them. Don't remove items; leave the ones that are not applicable unchecked._

I have:

- [ ] performed a self-review of my work (especially important for AI-assisted contributions).
- [ ] commented on the particularly hard-to-understand areas of my code.
- [ ] split my work into well-defined, bisectable commits, and I named my commits well
- [ ] formatted my code with **rustfmt** and avoided any unnecessary whitespace churn
- [ ] applied the appropriate labels (bug, enhancement, refactoring, documentation, etc.)
- [ ] checked that all my commits can be built.
- [ ] added or updated the on-line help insofar relevant for my changes
- [ ] confirmed that my code does not cause performance regressions (e.g. by running the Heretic timedemo).
- [ ] successfully executed the game regression test suite (```cargo test --release --test game_suite -- --ignored --nocapture```)
- [ ] added unit tests where applicable to prove the correctness of my code and to avoid future regressions.
- [ ] provided the release notes draft (for significant user-facing changes).

