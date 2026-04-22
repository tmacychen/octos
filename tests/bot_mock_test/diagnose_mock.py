#!/usr/bin/env python3
"""
Quick diagnostic: Check if Mock Server is running and can receive inject commands.
"""
import httpx
import sys

def check_mock_server():
    base_url = "http://127.0.0.1:5001"
    
    print("🔍 Checking Mock Server health...")
    try:
        resp = httpx.get(f"{base_url}/_health", timeout=2)
        if resp.status_code == 200:
            print("✅ Mock Server is running")
        else:
            print(f"❌ Mock Server returned {resp.status_code}")
            return False
    except Exception as e:
        print(f"❌ Cannot connect to Mock Server: {e}")
        return False
    
    print("\n📤 Testing inject endpoint...")
    try:
        resp = httpx.post(
            f"{base_url}/_inject",
            json={
                "text": "test message",
                "channel_id": "1039178386623557754",
                "sender_id": "123456789012345678",
                "username": "TestUser"
            },
            timeout=5
        )
        if resp.status_code == 200:
            print("✅ Inject endpoint works")
            print(f"   Response: {resp.json()}")
        else:
            print(f"❌ Inject endpoint returned {resp.status_code}: {resp.text}")
            return False
    except Exception as e:
        print(f"❌ Inject endpoint failed: {e}")
        return False
    
    print("\n📥 Checking sent messages...")
    try:
        resp = httpx.get(f"{base_url}/_sent_messages", timeout=2)
        if resp.status_code == 200:
            msgs = resp.json()
            print(f"✅ Found {len(msgs)} sent messages")
            if msgs:
                print(f"   Last message: {msgs[-1]['text'][:100]}")
        else:
            print(f"❌ Failed to get sent messages: {resp.status_code}")
    except Exception as e:
        print(f"❌ Error getting sent messages: {e}")
    
    return True

if __name__ == "__main__":
    success = check_mock_server()
    sys.exit(0 if success else 1)
