#!/usr/bin/env python3
"""
Diagnostic script to verify Discord Mock Server Gateway event dispatch.
Tests the actual behavior after code changes.
"""

import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import httpx
from websockets import connect as ws_connect


async def test_gateway_dispatch():
    """Test if Mock Server properly dispatches MESSAGE_CREATE events."""
    
    base_url = "http://127.0.0.1:5001"
    ws_url = "ws://127.0.0.1:5001/"
    
    print("=" * 80)
    print("🔍 Discord Mock Server Gateway Diagnostic Test")
    print("=" * 80)
    
    # Step 1: Check if server is running
    print("\n1️⃣  Checking Mock Server health...")
    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(f"{base_url}/health", timeout=2.0)
            if response.status_code == 200:
                print("   ✅ Mock Server is running and healthy")
            else:
                print(f"   ❌ Mock Server returned status {response.status_code}")
                return False
    except Exception as e:
        print(f"   ❌ Cannot connect to Mock Server: {e}")
        print("   💡 Start it with: python mock_discord.py")
        return False
    
    # Step 2: Connect to Gateway
    print("\n2️⃣  Connecting to Gateway WebSocket...")
    try:
        async with ws_connect(ws_url) as ws:
            # Receive HELLO
            hello_raw = await asyncio.wait_for(ws.recv(), timeout=2.0)
            hello = json.loads(hello_raw)
            print(f"   ✅ Received HELLO (op={hello['op']})")
            
            # Send IDENTIFY
            identify = {
                "op": 2,
                "d": {
                    "token": "fake-bot-token",
                    "intents": 32767,
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
            ready_raw = await asyncio.wait_for(ws.recv(), timeout=2.0)
            ready = json.loads(ready_raw)
            if ready["op"] == 0 and ready["t"] == "READY":
                print(f"   ✅ Received READY (session_id={ready['d']['session_id'][:20]}...)")
            else:
                print(f"   ❌ Expected READY, got op={ready['op']}, t={ready.get('t')}")
                return False
            
            # Step 3: Send a message via REST API
            print("\n3️⃣  Sending message via REST API...")
            channel_id = "1039178386623557754"
            message_content = "Test message for Gateway dispatch"
            
            async with httpx.AsyncClient() as client:
                response = await client.post(
                    f"{base_url}/api/v10/channels/{channel_id}/messages",
                    json={"content": message_content}
                )
                
                if response.status_code == 200:
                    rest_result = response.json()
                    message_id = rest_result["id"]
                    print(f"   ✅ REST API accepted (message_id={message_id})")
                else:
                    print(f"   ❌ REST API failed: {response.status_code}")
                    print(f"      Response: {response.text[:200]}")
                    return False
            
            # Step 4: Wait for MESSAGE_CREATE event
            print("\n4️⃣  Waiting for MESSAGE_CREATE event via Gateway...")
            print("   ⏳ Listening for events (timeout: 3 seconds)...")
            
            try:
                event_raw = await asyncio.wait_for(ws.recv(), timeout=3.0)
                event = json.loads(event_raw)
                
                print(f"\n   📨 Received event:")
                print(f"      - op: {event['op']}")
                print(f"      - t: {event.get('t', 'N/A')}")
                print(f"      - s: {event.get('s', 'N/A')}")
                
                if event["op"] == 0 and event["t"] == "MESSAGE_CREATE":
                    payload = event["d"]
                    print(f"\n   ✅ MESSAGE_CREATE event received!")
                    print(f"      - Message ID: {payload['id']}")
                    print(f"      - Content: {payload['content'][:60]}")
                    print(f"      - Author: {payload['author']['username']} (bot={payload['author']['bot']})")
                    print(f"      - Channel: {payload['channel_id']}")
                    
                    if payload["id"] == message_id:
                        print(f"\n   ✅✅✅ SUCCESS! Message ID matches REST API response")
                        print(f"   ✅✅✅ Gateway event dispatch is WORKING correctly!")
                        return True
                    else:
                        print(f"\n   ⚠️  WARNING: Message ID mismatch")
                        print(f"      Expected: {message_id}")
                        print(f"      Got: {payload['id']}")
                        return False
                else:
                    print(f"\n   ❌ Wrong event type: op={event['op']}, t={event.get('t')}")
                    print(f"   ❌ Expected: op=0, t=MESSAGE_CREATE")
                    return False
                    
            except asyncio.TimeoutError:
                print("\n   ❌❌❌ TIMEOUT! No MESSAGE_CREATE event received within 3 seconds")
                print("\n   🔍 This indicates one of the following issues:")
                print("      1. Mock Server is using OLD CODE (cached .pyc files)")
                print("      2. _dispatch_bot_message_via_gateway() method not implemented")
                print("      3. asyncio.create_task() not called in create_message endpoint")
                print("      4. WebSocket connection closed before event dispatched")
                print("\n   💡 Suggested fixes:")
                print("      - Clear Python cache: find . -name '*.pyc' -delete")
                print("      - Restart Mock Server completely")
                print("      - Verify mock_discord.py has _dispatch_bot_message_via_gateway method")
                return False
                
    except Exception as e:
        print(f"\n   ❌ Error during test: {type(e).__name__}: {e}")
        import traceback
        traceback.print_exc()
        return False


async def main():
    """Run diagnostic test."""
    result = await test_gateway_dispatch()
    
    print("\n" + "=" * 80)
    if result:
        print("🎉 DIAGNOSTIC PASSED - Gateway dispatch is working!")
    else:
        print("💥 DIAGNOSTIC FAILED - Gateway dispatch is NOT working!")
        print("\n📋 Next steps:")
        print("   1. Check if mock_discord.py was modified correctly")
        print("   2. Clear all .pyc cache files")
        print("   3. Restart Mock Server from scratch")
        print("   4. Run this diagnostic again")
    print("=" * 80)
    
    sys.exit(0 if result else 1)


if __name__ == "__main__":
    asyncio.run(main())
