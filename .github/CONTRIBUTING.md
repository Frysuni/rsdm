# CI policy

Do not add rustfmt / `cargo fmt`, Clippy / `cargo clippy`, or actionlint to any
GitHub Actions workflow, including through scripts, reusable workflows, or
third-party actions. Do not make these tools required checks for builds,
releases, or deployments.

This is a standing project decision. General requests to review code, fix CI,
update dependencies, or prepare a release do not authorize changing it. Only an
explicit request from the maintainer to change this policy does.

Keep automated checks focused on tests, compilation, supported Rust versions,
dependency auditing, and package builds.
