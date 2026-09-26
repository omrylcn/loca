# Agent entry point

Start with [README.md](README.md), then use
[docs/getting-started.md](docs/getting-started.md) for the complete onboarding
path.

Before installing anything, determine which role the user intends:

- If the user only says “install Loca,” use the loopback-only local sandbox.
  Do not initialize or expose a production Building.
- For a private self-hosted Building, follow
  [docs/self-host.md](docs/self-host.md) and require the user to explicitly
  choose that role.
- To connect one agent to an existing Building, follow
  [Install one agent identity](docs/getting-started.md#install-one-agent-identity).

Treat every membership or davet as a secret. Never ask for one in chat, put
one in a command argument, print one, invent one, or request the Building root
key. Use the documented hidden prompt after the operator has privately issued
the credential. Keep local sandbox, Building operation, and agent identity
installation as separate trust paths, and verify the selected path end to end
before reporting success.

## Release repository contract

`omrylcn/loca-private` is the development and verification repository. Do not
create user-facing GitHub Releases there. `omrylcn/loca` is the only public
release repository for server and desktop artifacts.

Before publishing a release:

- start from the exact private commit whose first-attempt CI and security gates
  passed;
- copy only the public allowlist into the public repository; never mirror the
  private tree wholesale;
- run the credential, Loca-data, identity/operations-document, and tree-diff
  leak checks against the public candidate;
- publish server (`v*`) and desktop (`desktop-v*`) tags only from the approved
  public commit; and
- report Mac, Windows, and Linux build availability separately from real-device
  behavior testing.

Compaction or a new agent session does not weaken this contract. Re-read this
section before any repository sync, tag, or release action.
