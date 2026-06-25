# Backend Next Migration Status

Updated: 2026-06-17

## Current State

- Rust workspace compiles and passes the full automated test suite.
- Route-level parity is tracked by `backend_next/tools/route-parity.mjs`.
- Latest route inventory: 469 Go routes, 530 Rust routes, 469 exact matches, and 0 missing Rust routes.
- Gateway supports OpenAI Responses, Chat Completions, Responses-over-Chat bridge with LLM compaction, Anthropic Messages, Gemini, embeddings, images, Antigravity aliases, WebSocket passthrough, account selection, failover, quota checks, usage logging, and risk-control preflight.
- Repository-backed coverage includes users, API keys, auth credentials, auth sessions, OAuth identities, groups, accounts, group bindings, usage logs, payment data, admin collections, settings, channel monitor history, user attributes, subscriptions, content moderation logs, affiliate profile/invite/rebate/transfer state, and system operation records.

## Recently Completed

- TOTP secrets are encrypted at rest with AES-256-GCM.
- `totp.encryption_key` and `TOTP_ENCRYPTION_KEY` are supported by `AppConfig`.
- Admin/public settings report whether the TOTP encryption key is configured.
- Enabling global TOTP without a configured encryption key is rejected.
- `/setup/test-db` now performs a real PostgreSQL connection and `SELECT 1`, instead of only validating the shape of the config.
- User refund eligible payment providers are derived from configured payment provider instances where `refund_enabled` and `allow_user_refund` are both true.
- External smoke covers the real local PostgreSQL and Redis dependencies requested for migration testing.
- `/api/v1/settings/email-unsubscribe` validates Go-compatible signed unsubscribe tokens, rejects transactional events, returns the HTML success page, and persists optional-notification preferences.
- User extra notification email verification now has an in-memory lifecycle with 15 minute TTL, 1 minute resend cooldown, 5 failed-attempt limit, 10 minute per-user send-rate window, max 3 verified notification emails, and `EMAIL_NOT_FOUND` handling for toggle/remove.
- Email account binding now requires a generated verification code plus the current password; codes use the same TTL, cooldown, failed-attempt, and per-user rate window as notification email verification.
- Email verification codes now use a pluggable store with in-memory and Redis backends. Redis keys are Go-compatible: `verify_code:<email>`, `notify_verify:<email>`, and `notify_code_user_rate:<userID>`.
- TOTP email verification now reuses the user's primary email verification code, matching the Go backend's `verify_code:<email>` behavior instead of keeping a separate user-ID keyed code.
- Send-code routes now deliver verification email through configured SMTP before committing the verification code. Failed delivery returns an error and does not persist a usable code.
- Password reset now uses 64-character random one-time tokens, Go-compatible Redis keys (`password_reset:<email>` and `password_reset_sent:<email>`), 30 minute token TTL, 30 second email cooldown, SMTP reset-link delivery, feature-toggle enforcement, anti-enumeration behavior, and session revocation after a successful reset.
- External smoke now covers Redis-backed verification-code and password-reset storage in addition to PostgreSQL and the Responses chat-state Redis store.
- User affiliate transfers now claim available affiliate quota once, reject empty transfers, credit the repository-backed balance ledger, retain quota snapshots in transaction metadata, and expose the updated balance through `/auth/me` and `/user/profile`.
- Affiliate admin profile updates, batch rebate-rate resets, user detail reads, invite binding, payment-triggered rebate accrual, and transfer/rebate reports now use repository-backed state and survive app recreation.
- Affiliate rebate behavior now covers Go-compatible default rate, per-invitee rebate caps, freeze/thaw windows, lazy thaw through user/admin reads and transfer, rebate duration limits, admin rate validation, and balance-redeem-triggered affiliate rebates.
- Affiliate admin invite/rebate/transfer reports now support repository-backed search, pagination, time-window filtering, and common `sort_by`/`sort_order` fields.
- OAuth pending browser sessions now use structured in-memory session state, Go-style pending cookies, cookie-only pending completion, browser-session matching, one-time consumption, create-account identity binding, bind-current-user cookie flow, bind-login credential validation, and rejection of forged/reused pending tokens.
- OAuth identities are now repository-backed with PostgreSQL migration support, in-memory/Postgres implementations, cross-user external identity conflict protection before session consumption, and route tests covering create-account, bind-login, configured provider callbacks, and app recreation.
- Auth sessions are now repository-backed with PostgreSQL migration support. Register/login/2FA/OAuth/refresh issue paths persist access/refresh sessions, refresh tokens are one-time across recreated app state, logout/reset-password/revoke-all-sessions revoke durable sessions, and `/auth/me` can recover the current user from a repository-backed access token after app recreation.
- Auth credentials are now repository-backed with PostgreSQL migration support. Registration and OAuth account creation persist password credentials, login can recover users after app recreation, `/api/v1/user/password` can update repository-backed credentials after in-memory auth state is lost, and password reset can generate/consume reset tokens for repository-backed users.
- Demo and production fallback state now seed the admin user/API key/credential into the repository, and PostgreSQL production startup upserts the admin user plus password credential after migrations.
- Verification-code and password-reset email delivery now renders persisted admin email templates, supports default template fallback keys, and exposes `notification_email.verify_code` in the admin template catalog.
- A modular async email queue component now supports bounded enqueue, worker fan-out, retry attempts, failure/full statistics, graceful shutdown, optional repository-backed durable task persistence, startup recovery, and admin status/manual recovery endpoints. It is wired into verification-code and password-reset routes behind explicit `email_queue` / `BACKEND_NEXT_EMAIL_QUEUE_*` configuration while preserving synchronous delivery by default.
- Backup creation now goes through a modular backup executor boundary and stores a repository manifest snapshot for migrated admin settings, backup/data-management/risk-control config, email templates, notification preferences, and repository-backed admin collections. Download URLs now issue one-hour repository-backed tokens and stream either the manifest snapshot or the PostgreSQL gzip artifact without requiring browser Authorization headers. Restore requires the current administrator password, replaces the backed-up scope from the manifest, and is covered by download, configuration/profile rollback, and missing/incorrect password tests. An opt-in `postgres_s3` executor can now run `pg_dump`, gzip the SQL dump, upload it to S3-compatible storage, download the artifact, restore it through `psql`, and delete the remote object with the backup record while preserving the existing repository-manifest backup as the default.
- Backup scheduling now has a repository-backed orchestration path: schedules persist `last_run_at` / `next_run_at`, a due run creates the configured backup through the same executor boundary, and a PostgreSQL consistency lock prevents multiple instances from creating the same scheduled backup. Redis is treated as cache-only and is not restored; filesystem restore is intentionally out of scope for this version.
- Data-management agent health now reads repository-backed configuration, reports configured enabled/status/version/pid/socket-path metadata, and survives app-state recreation instead of returning a hardcoded in-memory disabled response.
- Data-management backup job creation now stores repository-backed completed jobs with idempotency keys, source/S3 profile references, redacted execution-plan metadata, target artifact paths, S3 object keys, generated tar.gz artifacts, local artifact writes, SHA-256/size metadata, query filtering/pagination, and optional S3 upload through the shared SigV4 client. PostgreSQL jobs can include `pg_dump` SQL gzip output in the artifact, Redis jobs can export key/value data as JSONL with key type, TTL, and base64-encoded binary-safe values, and full jobs can include explicitly configured filesystem asset paths in the same artifact while skipping files above the configured size limit.
- Admin ops request, upstream-error, realtime traffic, account availability, concurrency shape, and OpenAI token-stat views now ingest recorded gateway outcomes, including selected account/provider, requested/upstream models, status code, latency, and token usage for successful non-stream JSON responses.
- Admin ops concurrency and account availability now merge live in-process gateway scheduler snapshots, so active account in-flight usage, configured account concurrency capacity, load percentage, and failure-cooldown state are visible before the request is persisted to the historical request log. Streaming gateway responses keep their account slot until the downstream response body is drained or dropped.
- PostgreSQL now provides the first shared consistency backend for multi-instance correctness: `distributed_locks` with leases/fencing tokens, `idempotent_jobs` with unique creation and leased claiming, `account_concurrency_slots` for global account in-flight limits, and `rate_limit_counters` for fixed-window count/usage counters. Gateway account concurrency and API-key 5h/1d/7d short rate-limit usage use the PostgreSQL backend when the production repository is active and keep the in-memory scheduler/auth counters as the demo/test fallback.
- API-key quota and rate-limit configuration is now part of the repository `api_keys` record, including `quota`, `quota_used`, 5h/1d/7d limits, usage counters, and window starts. Repository-only API keys can authenticate gateway requests, display quota state through `/v1/usage`, and persist successful gateway cost usage back to PostgreSQL.
- Admin ops alert rules now execute against gateway request/error metrics when gateway events are recorded. The evaluator supports rule filters, operators, threshold comparison, cooldown suppression, and firing alert-event creation for frontend metric types such as success/error rate, upstream error rate/count, group/account availability/error/rate-limit metrics, token consumption, and duration p95.
- Admin ops seed/demo telemetry is now limited to demo/test state; production `AppState` starts with empty ops request/error/log buffers so monitoring pages do not show synthetic errors before real traffic arrives.
- Admin risk-control API-key testing now performs real `/v1/moderations` probes against the configured or request-supplied base URL/model/API keys, returns per-key status/HTTP/latency details, and builds audit results from returned category scores and configured thresholds.
- Admin risk-control unban now updates repository-backed user status to `active` and is covered across app-state recreation.
- Admin system update/rollback/restart routes now use a modular repository-backed system operation layer with Go-compatible version/check-update response fields, GitHub latest-release checks with repository cache and cached fallback, release asset selection, trusted download URL validation, checksum verification, archive extraction, controlled binary replacement, rollback from backup binaries, operation IDs, idempotency-key replay, persisted operation results, app-recreation survival, and global running-operation conflict checks. Restart is recorded without actually restarting the test process.
- Configurable OAuth provider callbacks can now exchange authorization codes against configured token endpoints, fetch provider profiles, and seed pending OAuth sessions with the real provider subject/email while preserving the old demo fallback when a provider is not configured.
- Dashboard trend buckets now use the requested/default dashboard timezone consistently with the date filters, and tests cover the cross-day boundary behavior.

## Verification

Latest local verification:

```bash
cd backend_next
cargo fmt -- --check
cargo check
cargo test -- --nocapture --test-threads=1
node .\tools\route-parity.mjs
```

Latest external dependency smoke:

```bash
cd backend_next
BACKEND_NEXT_EXTERNAL_DEPS=1 \
DATABASE_URL='postgres://test:123456@127.0.0.1:5432/sub2api_new?sslmode=disable' \
REDIS_HOST=127.0.0.1 \
REDIS_PORT=6379 \
REDIS_PASSWORD='123456' \
REDIS_DB=0 \
REDIS_ENABLE_TLS=false \
BACKEND_NEXT_RESPONSES_STATE_STORE=redis \
BACKEND_NEXT_VERIFICATION_CODE_STORE=redis \
BACKEND_NEXT_PASSWORD_RESET_STORE=redis \
cargo test external_postgres_and_redis -- --nocapture --test-threads=1
```

## Remaining High-Value Gaps

- OAuth provider callbacks have a configurable token/profile exchange path and durable identity bindings. Remaining production refinements are provider-specific profile field quirks and broader LinuxDo/GitHub/Google/WeChat/OIDC/DingTalk coverage.
- Async email queue behavior has a tested in-process queue component, route-level opt-in for send-code/password-reset delivery, repository-backed durable persistence/recovery, and admin queue status/manual recovery controls. Remaining production refinements are mostly multi-instance leasing and delayed scheduling.
- Affiliate profiles, invite binding, payment/redeem-triggered rebate accrual, freeze/thaw windows, per-invitee caps, quota transfers, and admin reports are repository-backed. Remaining production refinements are mostly deeper audit parity and any edge-case frontend fields discovered during UI wiring.
- Backup/restore now handles repository-backed migrated state through explicit manifest snapshots, a pluggable executor boundary, signed downloads, admin-password restore confirmation, and scheduled backup orchestration with PostgreSQL locking. The backup executor layer also has an opt-in PostgreSQL-to-S3 path with `pg_dump`, gzip compression, SigV4 PUT/GET/DELETE, `psql` restore, route-level coverage, upload/download/delete verification, and fake-command restore verification. Data-management agent health reflects repository config, and data-management backup jobs generate auditable tar.gz artifacts with optional S3 upload, PostgreSQL SQL gzip dumps, Redis JSONL dumps, and explicitly configured filesystem asset inclusion. Redis is cache-only and filesystem restore is not planned for the current version; remaining production work is background timer wiring, schedule observability, retention cleanup execution, and data-management artifact restore UX/API for supported database artifacts.
- System operation APIs now expose durable operation IDs, idempotency replay, busy-operation conflicts, real latest-release checking with cache/fallback behavior, and an update executor for asset download/checksum/extract plus guarded binary replacement and backup rollback. Remaining production work is platform-specific service restart execution, release packaging hardening across deployment modes, and distributed atomic locking beyond the current repository record checks.
- Ops/admin observability routes now ingest real gateway request/error events and in-process scheduler snapshots for core request lists, realtime traffic summaries, account availability, live account in-flight concurrency for streaming and non-streaming gateway calls, cooldown status, OpenAI token stats, and in-process alert-rule evaluation. PostgreSQL-backed account concurrency slots and API-key fixed-window usage counters now cover the first multi-instance gateway limits. Remaining production work is backup scheduling orchestration, optional Redis acceleration/wait queues for very high QPS, durable ops event storage, alert-event email delivery, distributed/periodic alert evaluation, and richer dashboard metrics beyond the recorded request/error state.
- Risk-control gateway preflight, repository logs, flagged-hash storage, admin API-key test probes, and admin unban are backed by repository/API behavior. Remaining production refinements are richer runtime worker metrics, auth-cache invalidation side effects for distributed deployments, and operator-facing status details beyond the persisted log/hash state.
