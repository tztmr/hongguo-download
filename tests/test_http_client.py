import unittest
from urllib.parse import parse_qs, urlparse

import httpx

from core.device_pool import DeviceEntry
from core.http_client import PureSignedClient, _with_device_identity


class DeviceIdentityTests(unittest.IsolatedAsyncioTestCase):
    def test_with_device_identity_replaces_stale_values(self):
        device = DeviceEntry("new-device", "new-install", "secret")

        url = _with_device_identity(
            "https://example.test/path?aid=8662&device_id=old-device&iid=old-install",
            device,
        )

        query = parse_qs(urlparse(url).query)
        self.assertEqual(query["device_id"], ["new-device"])
        self.assertEqual(query["iid"], ["new-install"])

    async def test_signed_post_injects_identity_before_signing(self):
        seen = {}

        async def handle(request):
            seen["url"] = str(request.url)
            return httpx.Response(200, json={"code": 0})

        client = PureSignedClient()
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=httpx.MockTransport(handle))
        try:
            await client.signed_post(
                "https://example.test/path?aid=8662",
                "{}",
                content_type="application/json",
                aid=8662,
                device=DeviceEntry("device-1", "install-1", "secret"),
            )
        finally:
            await client.close()

        query = parse_qs(urlparse(seen["url"]).query)
        self.assertEqual(query["device_id"], ["device-1"])
        self.assertEqual(query["iid"], ["install-1"])


if __name__ == "__main__":
    unittest.main()
