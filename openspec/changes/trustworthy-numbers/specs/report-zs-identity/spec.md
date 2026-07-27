## ADDED Requirements

### Requirement: A run records the zerostack identity or does not produce a report
At run start — once per invocation, before any trial spends money — the harness SHALL capture the zerostack identity and record it in `Report`:

- `zs_version`: the first line of `ZS_BIN --version` stdout, verbatim. Capture succeeds iff the process exits 0 with non-empty stdout. **No format validation**: the version string is evidence for humans; validating its shape would turn upstream's output format into a compatibility contract exactly while we ask upstream to enrich it (issue candidate: embed git sha + features via build.rs, motivated by the live 07-26 incident of a binary printing 1.7.1 against a 1.7.2 checkout).
- `zs_bin_path`: the binary's path as resolved.
- `zs_bin_sha256`: sha256 of the binary's file contents, computed once per run, never per trial. The machine-comparable identity — two builds printing the same version stay distinguishable, following the pack path+fingerprint precedent.

Any capture failure — unrunnable binary, nonzero exit, empty output — SHALL abort the run. This feature exists to make identity-less reports impossible; a null fallback would reintroduce them.

#### Scenario: A healthy binary is recorded verbatim
- **WHEN** `run` starts against a `--zs-bin` whose `--version` prints `zerostack 1.7.2` and exits 0
- **THEN** report.json records `zs_version = "zerostack 1.7.2"`, the binary path, and its content sha256

#### Scenario: Nonzero exit aborts the run
- **WHEN** the binary exits nonzero on `--version`
- **THEN** the run aborts before any trial, with an error naming the binary

#### Scenario: Empty output aborts the run
- **WHEN** the binary exits 0 but prints nothing
- **THEN** the run aborts

#### Scenario: Missing binary aborts the run
- **WHEN** the `--zs-bin` path does not exist or is not executable
- **THEN** the run aborts

#### Scenario: Multi-line output records the first line
- **WHEN** a future binary prints extra lines after the version line
- **THEN** capture succeeds and the first line is recorded verbatim

### Requirement: git_sha and features are honest placeholders
`Report` SHALL carry `git_sha` and `features` as optional fields that are `null` today: live-tested 07-26, the binary embeds neither (no build.rs, no clap version customization), so there is no runtime "record if available" branch — the spec states the fact. When upstream embeds build info, these fields connect without a schema change.

#### Scenario: Nulls are recorded, not omitted
- **WHEN** any run completes today
- **THEN** report.json contains `"git_sha": null` and `"features": null` — present and null, never absent, never fabricated

### Requirement: Mock runs record fixture identity
The identity fields record what produced the run's evidence. For `--backend mock=<fixture>` that is the fixture, not a binary; a mock run SHALL record `zs_version = "mock"` (the backend's existing name), `zs_bin_path` = the fixture path as given, and `zs_bin_sha256` = the fixture's content fingerprint — a single file hashes its bytes; a directory folds its files sorted by relative path with length-prefixed fields (deliberately not copying `PromptPack::fingerprint`'s registered NUL-collision flaw). A `--zs-bin` passed alongside a mock backend stays ignored, as it is today: identity records the evidence source, not an unused binary.

Consequence, by design: two mock reports from different fixtures differ in `zs_bin_sha256` and trip compare's build-mismatch warning — different fixtures are different subjects — giving the harness's own tests a zero-API path through the mismatch logic.

#### Scenario: File fixture identity
- **WHEN** a mock run replays a single session JSON fixture
- **THEN** the report records `zs_version = "mock"`, the fixture path, and the sha256 of the fixture file's bytes

#### Scenario: Directory fixture identity is content-based
- **WHEN** two mock runs replay directory fixtures with identical contents at different paths
- **THEN** both reports record the same `zs_bin_sha256`

#### Scenario: Different fixtures are different subjects
- **WHEN** two mock runs replay fixtures with differing contents
- **THEN** their `zs_bin_sha256` values differ
