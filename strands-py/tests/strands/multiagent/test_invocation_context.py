"""Tests for request-wide invocation context accounting across Graph and Swarm nodes."""

import pytest

from strands import Agent
from strands.agent.invocation_context import InvocationContext
from strands.hooks import AfterInvocationEvent
from strands.multiagent.base import Status
from strands.multiagent.graph import GraphBuilder
from strands.multiagent.swarm import Swarm
from strands.types.event_loop import _limit_stop_message
from tests.fixtures.mocked_model_provider import MockedModelProvider


def _usage(input_tokens, output_tokens):
    return {"inputTokens": input_tokens, "outputTokens": output_tokens, "totalTokens": input_tokens + output_tokens}


def _text_agent(name, text, usage):
    return Agent(
        model=MockedModelProvider([{"role": "assistant", "content": [{"text": text}]}], [usage]),
        name=name,
        callback_handler=None,
    )


@pytest.mark.asyncio
async def test_graph_folds_every_node_usage_and_turn_into_one_shared_request_total():
    agent_a = _text_agent("a", "A done", _usage(10, 25))
    agent_b = _text_agent("b", "B done", _usage(20, 35))
    shared = []
    agent_b.hooks.add_callback(AfterInvocationEvent, lambda event: shared.append(event.invocation_context))
    builder = GraphBuilder()
    builder.add_node(agent_a, "a")
    builder.add_node(agent_b, "b")
    builder.add_edge("a", "b")
    graph = builder.build()

    result = await graph.invoke_async("hello")

    assert (shared[0].turns_taken, shared[0].usage) == (2, {"inputTokens": 30, "outputTokens": 60, "totalTokens": 90})
    assert result.invocation_context is shared[0]


@pytest.mark.asyncio
async def test_graph_adds_its_usage_to_the_request_when_it_runs_inside_a_nested_call_own_limits():
    builder = GraphBuilder()
    builder.add_node(_text_agent("analyst", "done", _usage(2, 3)), "analyst")
    graph = builder.build()
    request = InvocationContext()
    request._record_usage(_usage(100, 100))
    nested = InvocationContext({"turns": 5}, request)

    result = await graph.invoke_async("analyze", invocation_context=nested)

    assert result.invocation_context is nested
    assert (nested.turns_taken, nested.usage) == (1, {"inputTokens": 2, "outputTokens": 3, "totalTokens": 5})
    assert (request.turns_taken, request.usage) == (1, {"inputTokens": 102, "outputTokens": 103, "totalTokens": 205})


@pytest.mark.asyncio
async def test_graph_fails_a_node_whose_agent_stops_on_the_forwarded_request_limit():
    builder = GraphBuilder()
    builder.add_node(_text_agent("analyst", "never reached", _usage(0, 0)), "analyst")
    graph = builder.build()
    spent_request = InvocationContext({"turns": 1})
    spent_request._turns_taken = 1

    result = await graph.invoke_async("analyze", invocation_context=spent_request)

    node_result = result.results["analyst"]
    assert node_result.status == Status.FAILED
    assert isinstance(node_result.result, Exception)
    assert str(node_result.result) == _limit_stop_message("analyst", "limit_turns")
    assert result.status == Status.FAILED


def _handoff_agent(name, next_agent, usage):
    """Agent that hands off to ``next_agent`` via the handoff tool, or finishes when it is ``None``."""
    if next_agent is None:
        responses = [{"role": "assistant", "content": [{"text": f"{name} done"}]}]
        usages = [usage]
    else:
        responses = [
            {
                "role": "assistant",
                "content": [
                    {
                        "toolUse": {
                            "toolUseId": f"tool-{name}",
                            "name": "handoff_to_agent",
                            "input": {"agent_name": next_agent, "message": f"from {name}"},
                        }
                    }
                ],
            },
            # The Python handoff tool is a regular tool, so the agent takes one more turn after it.
            {"role": "assistant", "content": [{"text": f"{name} handed off"}]},
        ]
        usages = [usage, _usage(0, 0)]
    return Agent(model=MockedModelProvider(responses, usages), name=name, callback_handler=None)


@pytest.mark.asyncio
async def test_swarm_folds_every_node_usage_into_one_shared_request_total():
    agent_a = _handoff_agent("a", "b", _usage(10, 25))
    agent_b = _handoff_agent("b", None, _usage(20, 35))
    shared = []
    agent_b.hooks.add_callback(AfterInvocationEvent, lambda event: shared.append(event.invocation_context))
    swarm = Swarm([agent_a, agent_b])

    result = await swarm.invoke_async("hello")

    assert (shared[0].turns_taken, shared[0].usage) == (3, {"inputTokens": 30, "outputTokens": 60, "totalTokens": 90})
    assert result.invocation_context is shared[0]


@pytest.mark.asyncio
async def test_swarm_fails_a_node_whose_agent_stops_on_the_forwarded_request_limit():
    swarm = Swarm([_text_agent("analyst", "never reached", _usage(0, 0))])
    spent_request = InvocationContext({"turns": 1})
    spent_request._turns_taken = 1

    result = await swarm.invoke_async("analyze", invocation_context=spent_request)

    node_result = result.results["analyst"]
    assert node_result.status == Status.FAILED
    assert str(node_result.result) == _limit_stop_message("analyst", "limit_turns")
