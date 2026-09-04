#!/bin/bash
# Runs Playwright MCP server on the Mac Mini via SSH
# The browser opens headed on the Mini so the operator can watch
# Uses globally installed binary to avoid npx startup delay
exec ssh -o StrictHostKeyChecking=no mini \
  "export PATH=/Users/franciscogoncalves/.local/share/fnm/node-versions/v22.22.2/installation/bin:/opt/homebrew/bin:/opt/homebrew/sbin:\$PATH && \
   playwright-mcp --isolated --browser chromium --viewport-size 1280x720"
