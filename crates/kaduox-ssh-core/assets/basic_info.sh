#!/bin/sh
# Read-only basic host snapshot shared by the CLI/MCP/desktop wire format.
# Emits __KADUOX_BASIC_INFO_V1__ followed by key=value lines.
printf '__KADUOX_BASIC_INFO_V1__\n'; printf 'hostname='; hostname 2>/dev/null; printf 'platform='; uname -srmo 2>/dev/null; printf 'username='; id -un 2>/dev/null; printf 'uptime='; uptime -p 2>/dev/null || uptime 2>/dev/null; printf 'addresses='; hostname -I 2>/dev/null
