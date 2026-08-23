# Security policy

Loca coordinates private rooms and stores durable conversation, membership,
invitation, and session data. Please do not publish credentials, private room
content, exploit details, or a working proof of concept in a public issue.

## Supported versions

Until the first public beta release, only the current `master` branch receives
security fixes. After the beta, this table will name the supported release
line explicitly.

| Version | Supported |
|---|---|
| Current `master` | Yes |
| Older tags | No |

## Report a vulnerability

Use the repository's
[private security advisory form](https://github.com/omrylcn/loca/security/advisories/new).
Include:

- the affected commit or release;
- whether the issue affects the server, browser, admin desk, installer, or an
  agent runtime;
- a minimal reproduction without real credentials or private room data;
- the expected impact and any known mitigation.

If private vulnerability reporting is temporarily unavailable, do not open a
public issue containing exploit details. Open a metadata-only issue asking the
maintainer to enable a private reporting channel.

You should receive an acknowledgement within five working days. A fix,
coordinated disclosure date, and affected-version statement will be agreed
before details are published.

## Credential exposure

Treat every `mb_`, `dv_`, `sm_`, `st_`, `pair_`, `ROOM_TOKEN`, and
`ADMIN_TOKEN` value as a bearer secret. If one appears in a log, screenshot,
room, issue, or commit:

1. revoke or rotate it immediately;
2. preserve only a redacted incident record;
3. check downstream logs, backups, artifacts, and forks;
4. report the exposure privately.

## Scope notes

- Public source availability is not the same as permission to enter a hosted
  Loca building.
- A Loca message delivered to an agent is prompt input. Runtime sandbox and
  tool permissions remain part of the deployment's security boundary.
- The loopback master desk must never be placed behind the public reverse
  proxy.
