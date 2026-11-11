# GitHub workflows, parked

These are the CI and release workflows, kept here so they travel with the repository. They are not active: GitHub only runs workflows from `.github/workflows/`.

They were left out of `.github/workflows/` because the login used for the first push did not have GitHub's `workflow` permission, and the account had no Actions minutes left that month.

To turn them on:

```sh
gh auth refresh -h github.com -s workflow
mkdir -p .github/workflows
git mv docs/workflows/ci.yml docs/workflows/release.yml .github/workflows/
git commit -m "Turn on CI and release workflows"
git push
```

- `ci.yml` runs rustfmt, clippy, the tests, and shellcheck on every push and pull request.
- `release.yml` builds the Linux archive when a `v*` tag is pushed. Neither has ever run.

Until then, build a release by hand on x86_64 Linux with `scripts/build-release.sh`.
