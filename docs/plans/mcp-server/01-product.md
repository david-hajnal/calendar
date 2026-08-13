# Product: MCP Server — AI Calendar Integration

## Problem
Users want their AI assistants (Claude, ChatGPT, etc.) to read and manage their calendar events — check availability, create meetings, find appointments — but they won't hand over their calendar credentials to an AI. They need a secure way to let AI helpers do calendar work without giving those helpers full access to their account.

## Success metric
Number of connected MCP clients per active user (target: 2.0+ by launch + 30 days), measured from McpGrant table.

## Announcement — the blog post before the feature
Your calendar is the most personal part of your digital day. Now let your AI assistant help manage it — safely. CommonCal's new MCP integration lets you connect Claude, ChatGPT, or any compatible AI tool to your calendar with granular, per-client permissions. Choose which calendars each AI can see. Pick exactly what it can do. Revoke access instantly. Your AI reads your schedule, creates events, and finds free slots — without ever holding your password or getting blanket access to everything.

## Screens
- mockups/01-mcp-connection.html — OAuth connection flow: "Claude wants access to CommonCal" with calendar checkboxes and permission toggles
- mockups/02-mcp-settings.html — Settings → AI & MCP connections: list of connected clients with permissions and revoke
- mockups/03-mcp-confirm.html — Deletion confirmation page: "AI client wants to delete event" with cancel/delete buttons
