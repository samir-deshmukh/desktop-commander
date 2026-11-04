# In-app AI Agent

The AI Agent page accepts a natural-language task and lets the configured chat-completion model decide which Desktop Commander MCP tools to call and in what order. It is not a separate PDF or text-edit workflow.

## Execution path

1. The UI sends the task to the configured OpenAI-compatible `/chat/completions` endpoint with the MCP tool definitions.
2. The model may respond with one or more function tool calls.
3. The Tauri host forwards each tool call to the local `sam-desktop-mcp` executable over its STDIO JSON-RPC protocol. The host performs MCP initialization, discovers tools, and calls the requested tool.
4. Tool results return to the model. The model can make additional tool calls or provide a final response.
5. The agent repeats this loop for at most 12 model turns and reports when the limit is reached.

The MCP process is started for each discovery or tool call, so the desktop UI does not need to keep a second MCP session alive. Calls use the same local permission file and MCP handlers as an external MCP client.

## Configuration and boundaries

- Configure the API endpoint, model, and API key in the AI Agent page. The key remains in React session state and is not written to app settings or project files.
- Task text and MCP results are sent to the selected model provider. Do not submit confidential material unless the provider is appropriate for it.
- Approved folder and writable-folder rules are still enforced by MCP. The agent cannot expand those permissions itself.
- Command execution remains disabled unless the owner enables it in MCP settings; interactive shell and process operations remain subject to the MCP server's existing controls.
- The AI Agent needs the standalone MCP executable built. The MCP server binary is not bundled into the frontend build.
- The agent does not add generic keyboard/mouse automation or screenshot access. It can use only the tools exposed by this MCP server.

## Verification

The frontend type-check/build and lint commands cover the agent UI. The existing `scripts/test-mcp-stdio.py` exercises the local MCP server protocol and tool dispatch. The model-provider loop requires a configured API endpoint and key for live end-to-end verification.
