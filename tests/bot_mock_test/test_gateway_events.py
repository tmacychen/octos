#!/usr/bin/env python3
"""
Test Discord Mock Server Gateway Event Dispatch

This test verifies that the Mock Discord Server properly dispatches
MESSAGE_CREATE events via Gateway WebSocket when the bot sends messages.
"""

import asyncio
import json
import sys
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent))

import httpx
from websockets import connect as ws_connect


async def test_gateway_message_dispatch():
    """Test that bot-sent messages trigger MESSAGE_CREATE events via Gateway."""
    
    base_url = "http://127.0.0.1:5001"
    ws_url = "ws://127.0.0.1:5001/"
    
    print("🧪 Testing Discord Mock Server Gateway Event Dispatch")
    print("=" * 70)
    
    # Step 1: Connect to Gateway WebSocket
    print("\n1️⃣  Connecting to Gateway WebSocket...")
    async with ws_connect(ws_url) as ws:
        # Receive HELLO
        hello = await ws.recv()
        hello_data = json.loads(hello)
        assert hello_data["op"] == 10, f"Expected OP 10 (HELLO), got {hello_data['op']}"
        print(f"   ✅ Received HELLO (heartbeat_interval={hello_data['d']['heartbeat_interval']}ms)")
        
        # Send IDENTIFY
        identify = {
            "op": 2,
            "d": {
                "token": "fake-bot-token",
                "intents": 32767,  # All intents
                "properties": {
                    "$os": "linux",
                    "$browser": "test",
                    "$device": "test",
                },
            }
        }
        await ws.send(json.dumps(identify))
        print("   ✅ Sent IDENTIFY")
        
        # Receive READY
        ready = await ws.recv()
        ready_data = json.loads(ready)
        assert ready_data["op"] == 0 and ready_data["t"] == "READY", \
            f"Expected READY event, got op={ready_data['op']}, t={ready_data.get('t')}"
        print(f"   ✅ Received READY (session_id={ready_data['d']['session_id'][:20]}...)")
        
        # Step 2: Send a message via REST API
        print("\n2️⃣  Sending message via REST API...")
        channel_id = "1039178386623557754"
        message_content = "Hello from bot!"
        
        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{base_url}/api/v10/channels/{channel_id}/messages",
                json={"content": message_content}
            )
            assert response.status_code == 200, f"REST API failed: {response.status_code}"
            
            rest_result = response.json()
            message_id = rest_result["id"]
            print(f"   ✅ REST API accepted message (id={message_id})")
        
        # Step 3: Wait for MESSAGE_CREATE event via Gateway
        print("\n3️⃣  Waiting for MESSAGE_CREATE event via Gateway...")
        try:
            event_raw = await asyncio.wait_for(ws.recv(), timeout=2.0)
            event_data = json.loads(event_raw)
            
            assert event_data["op"] == 0, f"Expected DISPATCH (op=0), got {event_data['op']}"
            assert event_data["t"] == "MESSAGE_CREATE", \
                f"Expected MESSAGE_CREATE, got {event_data['t']}"
            
            event_payload = event_data["d"]
            assert event_payload["id"] == message_id, \
                f"Message ID mismatch: expected {message_id}, got {event_payload['id']}"
            assert event_payload["content"] == message_content, \
                f"Content mismatch: expected '{message_content}', got '{event_payload['content']}'"
            assert event_payload["author"]["bot"] == True, \
                "Author should be marked as bot"
            
            print(f"   ✅ Received MESSAGE_CREATE event")
            print(f"      - Message ID: {event_payload['id']}")
            print(f"      - Content: {event_payload['content'][:50]}")
            print(f"      - Author: {event_payload['author']['username']} (bot={event_payload['author']['bot']})")
            print(f"      - Sequence: {event_data['s']}")
            
        except asyncio.TimeoutError:
            print("   ❌ FAILED: No MESSAGE_CREATE event received within 2 seconds")
            print("   💡 This indicates the Gateway event dispatch is not working")
            return False
    
    print("\n" + "=" * 70)
    print("✅ TEST PASSED: Gateway event dispatch is working correctly!")
    print("=" * 70)
    return True


async def test_multiple_messages():
    """Test that multiple messages each trigger separate MESSAGE_CREATE events."""
    
    base_url = "http://127.0.0.1:5001"
    ws_url = "ws://127.0.0.1:5001/"
    
    print("\n🧪 Testing Multiple Message Dispatch")
    print("=" * 70)
    
    async with ws_connect(ws_url) as ws:
        # Handshake
        await ws.recv()  # HELLO
        await ws.send(json.dumps({
            "op": 2,
            "d": {
                "token": "fake-token",
                "intents": 32767,
                "properties": {"$os": "linux", "$browser": "test", "$device": "test"},
            }
        }))
        await ws.recv()  # READY
        
        # Send 3 messages
        channel_id = "1039178386623557754"
        messages = ["Message 1", "Message 2", "Message 3"]
        message_ids = []
        
        async with httpx.AsyncClient() as client:
            for i, content in enumerate(messages, 1):
                response = await client.post(
                    f"{base_url}/api/v10/channels/{channel_id}/messages",
                    json={"content": content}
                )
                msg_id = response.json()["id"]
                message_ids.append(msg_id)
                print(f"   📤 Sent message {i}: '{content}' (id={msg_id})")
                
                # Wait for corresponding MESSAGE_CREATE event
                try:
                    event_raw = await asyncio.wait_for(ws.recv(), timeout=2.0)
                    event_data = json.loads(event_raw)
                    
                    assert event_data["t"] == "MESSAGE_CREATE"
                    assert event_data["d"]["id"] == msg_id
                    assert event_data["d"]["content"] == content
                    
                    print(f"   ✅ Received MESSAGE_CREATE for message {i}")
                    
                except asyncio.TimeoutError:
                    print(f"   ❌ FAILED: No event for message {i}")
                    return False
    
    print("\n✅ All messages triggered MESSAGE_CREATE events correctly!")
    return True


async def main():
    """Run all tests."""
    print("\n" + "🔍 Discord Mock Server Gateway Tests".center(70))
    print()
    
    # Check if server is running
    try:
        async with httpx.AsyncClient() as client:
            response = await client.get("http://127.0.0.1:5001/health", timeout=2.0)
            if response.status_code != 200:
                print("❌ Mock server is not running or unhealthy")
                print("💡 Start it with: python mock_discord.py")
                sys.exit(1)
    except Exception:
        print("❌ Cannot connect to mock server at http://127.0.0.1:5001")
        print("💡 Start it with: python mock_discord.py")
        sys.exit(1)
    
    # Run tests
    results = []
    
    try:
        result1 = await test_gateway_message_dispatch()
        results.append(("Single Message Dispatch", result1))
    except Exception as e:
        print(f"\n❌ Test failed with exception: {e}")
        import traceback
        traceback.print_exc()
        results.append(("Single Message Dispatch", False))
    
    try:
        result2 = await test_multiple_messages()
        results.append(("Multiple Messages Dispatch", result2))
    except Exception as e:
        print(f"\n❌ Test failed with exception: {e}")
        import traceback
        traceback.print_exc()
        results.append(("Multiple Messages Dispatch", False))
    
    # Summary
    print("\n" + "📊 Test Summary".center(70))
    print("=" * 70)
    for name, passed in results:
        status = "✅ PASS" if passed else "❌ FAIL"
        print(f"  {status:10} | {name}")
    print("=" * 70)
    
    all_passed = all(passed for _, passed in results)
    if all_passed:
        print("\n🎉 All tests passed!")
        sys.exit(0)
    else:
        print("\n💥 Some tests failed")
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
