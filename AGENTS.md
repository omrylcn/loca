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
