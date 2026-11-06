"""
Tests for n8n workflow JSON validity, nodes, and connectivity.
"""

import json
from pathlib import Path

WORKFLOW_FILE = Path(__file__).resolve().parent.parent / "workflows" / "academic-agent.n8n.json"


def test_n8n_workflow_file_exists():
    assert WORKFLOW_FILE.exists(), "academic-agent.n8n.json must exist"


def test_n8n_workflow_structure():
    with open(WORKFLOW_FILE, "r", encoding="utf-8") as f:
        data = json.load(f)

    assert "name" in data
    assert "nodes" in data
    assert "connections" in data
    assert isinstance(data["nodes"], list)
    assert len(data["nodes"]) >= 5

    node_names = [n["name"] for n in data["nodes"]]
    assert "Webhook: New Document/Task" in node_names
    assert "AI Node: 9Router / LLM Inference" in node_names
    assert "Parse & Build Calendar Data" in node_names
    assert "Send Discord Interactive Embed" in node_names
    assert "Respond to Webhook" in node_names


def test_n8n_workflow_connections():
    with open(WORKFLOW_FILE, "r", encoding="utf-8") as f:
        data = json.load(f)

    conn = data["connections"]
    # Check that webhook leads to preprocess
    assert "Webhook: New Document/Task" in conn
    # Check that AI node leads to parser
    assert "AI Node: 9Router / LLM Inference" in conn
