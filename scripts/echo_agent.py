"""Minimal ACP echo agent for testing the dual-window client."""
import asyncio
from typing import Any
from acp import Agent, PromptResponse, run_agent
from acp.helpers import update_agent_message_text


class EchoAgent(Agent):
    async def initialize(self, protocol_version, **kw):
        return {"protocolVersion": protocol_version, "agentCapabilities": {}}

    async def new_session(self, cwd, mcp_servers=None, **kw):
        return {"sessionId": "echo-1"}

    async def prompt(self, prompt, session_id, **kw):
        for block in prompt:
            text = getattr(block, "text", str(block))
            await self._conn.session_update(
                session_id=session_id,
                update=update_agent_message_text(f"Echo: {text}"),
            )
        return PromptResponse(stop_reason="end_turn")

    async def load_session(self, cwd, session_id, mcp_servers=None, **kw): return None
    async def list_sessions(self, cursor=None, cwd=None, **kw): return {"sessions": []}
    async def set_session_mode(self, mode_id, session_id, **kw): return None
    async def set_session_model(self, model_id, session_id, **kw): return None
    async def set_config_option(self, config_id, session_id, value, **kw): return None
    async def authenticate(self, method_id, **kw): return None
    async def fork_session(self, cwd, session_id, mcp_servers=None, **kw): return {"sessionId": session_id}
    async def resume_session(self, cwd, session_id, mcp_servers=None, **kw): return {"sessionId": session_id}
    async def cancel(self, session_id, **kw): pass
    async def ext_method(self, method, params): return {}
    async def ext_notification(self, method, params): pass
    def on_connect(self, conn): self._conn = conn


asyncio.run(run_agent(EchoAgent()))
