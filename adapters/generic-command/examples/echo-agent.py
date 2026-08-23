#!/usr/bin/env python3
"""Smallest possible generic-command agent: echo the trigger back.

Demonstrates the whole contract in a few lines — read one JSON object from
stdin, write one JSON object to stdout.
"""
import json
import sys

req = json.load(sys.stdin)
trigger = req["trigger"]
print(json.dumps({
    "text": f"echo: {trigger.get('text', '')}",
    # target/reply_to are optional — agentd defaults them to the trigger's
    # sender/id; set them here only to address someone else.
}, ensure_ascii=False))
