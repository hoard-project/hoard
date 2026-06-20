# Governance

Hoard uses a **BDFL + Maintainer** model.

## Roles

### BDFL (Benevolent Dictator for Life)

The BDFL has final decision-making authority on all project matters.
Currently: **Ethan**.

### Maintainers

Maintainers have write access to the repository and can:

- Merge pull requests after review
- Cut releases
- Manage issues and discussions

Decisions among maintainers are made by **lazy consensus**: a proposal is
considered accepted if no maintainer objects within 72 hours. The BDFL
can override any decision.

### Contributors

Anyone who submits a merged pull request is a contributor. Contributors
are listed in the release notes and have no formal authority.

## Becoming a maintainer

1. Contribute consistently for ≥ 3 months
2. Demonstrate thoughtful code review on ≥ 5 PRs
3. Be nominated by an existing maintainer
4. Receive majority approval from existing maintainers

## Removing a maintainer

Maintainers who are inactive for ≥ 6 months may be moved to emeritus
status. The BDFL may remove a maintainer for Code of Conduct violations
with a 2/3 maintainer vote.

## Release process

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md`
3. Create a signed, annotated tag: `git tag -s vX.Y.Z -m "vX.Y.Z"`
4. Push the tag: `git push origin vX.Y.Z`
5. GitHub Actions builds and publishes the release

All releases follow [Semantic Versioning](https://semver.org/).

## Code of Conduct

All participants must follow the [Code of Conduct](CODE_OF_CONDUCT.md).
