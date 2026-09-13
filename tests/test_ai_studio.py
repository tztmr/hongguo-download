import asyncio
import base64
import json
import unittest
from unittest.mock import patch

import httpx
from fastapi import FastAPI
from endpoints.ai_studio import router


class StudioTests(unittest.TestCase):
    def call(self, action, body, handler, origin=None):
        async def run():
            app = FastAPI()
            app.include_router(router, prefix='/api')
            upstream = httpx.AsyncClient(transport=httpx.MockTransport(handler))
            with patch('endpoints.ai_studio.make_client', return_value=upstream):
                async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://localhost') as local:
                    return await local.post('/api/studio/' + action, json=body, headers={'origin': origin} if origin else {})
        return asyncio.run(run())

    def test_model_list_uses_only_selected_provider_key(self):
        def upstream(req):
            self.assertEqual(str(req.url), 'https://moyuu.cc/v1/models')
            self.assertEqual(req.headers['authorization'], 'Bearer image-test-key')
            return httpx.Response(200, json={'data': [{'id': 'nanobanana'}, {'id': 'gpt-image-1'}, {'id': 'nanobanana'}]})
        response = self.call('models', {'provider': 'moyuu', 'apiKey': 'image-test-key'}, upstream)
        self.assertEqual(response.json()['models'], ['gpt-image-1', 'nanobanana'])

    def test_upstream_error_does_not_echo_key_or_body(self):
        response = self.call('models', {'provider': 'moyuu', 'apiKey': 'secret-fixture'}, lambda _: httpx.Response(401, text='Bearer secret-fixture'))
        self.assertEqual(response.status_code, 401)
        self.assertEqual(response.json()['code'], 'AI_KEY_INVALID')
        self.assertNotIn('secret-fixture', response.text)

    def test_cross_origin_is_rejected_before_network(self):
        response = self.call('models', {'provider': 'moyuu', 'apiKey': 'secret'}, lambda _: self.fail('network called'), 'https://evil.example')
        self.assertEqual(response.status_code, 403)

    def test_unapproved_destination_and_key_in_url_rejected(self):
        for url in ['http://127.0.0.1:8000', 'https://evil.example/v1', 'https://jucodex.com/v1?key=secret', 'https://user:secret@jucodex.com/v1']:
            response = self.call('models', {'provider': 'jucodex', 'baseUrl': url, 'apiKey': 'secret'}, lambda _: self.fail('network called'))
            self.assertEqual(response.status_code, 400)

    def test_text_json_mode_and_schema_validation(self):
        result = {'title_candidates': [{'title': '真实剧情', 'angle': '冲突', 'evidence': '源字幕'}], 'recommended_title': '真实剧情', 'description': '简介', 'tags': ['短剧'], 'category_suggestion': '娱乐', 'cover_concepts': [{'headline': '真相', 'composition': '人物左侧', 'prompt': 'cinematic boardroom', 'negative_prompt': 'blur'}]}
        def upstream(req):
            payload = json.loads(req.content)
            self.assertEqual(payload['model'], 'deepseek-v4-flash')
            self.assertEqual(payload['response_format'], {'type': 'json_object'})
            self.assertEqual(req.headers['authorization'], 'Bearer text-only')
            return httpx.Response(200, json={'choices': [{'message': {'content': json.dumps(result)}}]})
        response = self.call('text', {'provider': 'deepseek', 'apiKey': 'text-only', 'model': 'deepseek-v4-flash', 'prompt': '给出 JSON 标题'}, upstream)
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()['result']['description'], '简介')
        broken = self.call('text', {'provider': 'deepseek', 'apiKey': 'text-only', 'model': 'deepseek-v4-flash', 'prompt': 'JSON'}, lambda _: httpx.Response(200, json={'choices': [{'message': {'content': '{}'}}]}))
        self.assertEqual(broken.status_code, 502)

    def test_reference_image_is_sent_as_multipart_not_silently_dropped(self):
        encoded = base64.b64encode(b'\x89PNG\r\n\x1a\nfixture').decode()
        def upstream(req):
            self.assertTrue(str(req.url).endswith('/images/edits'))
            self.assertIn('multipart/form-data', req.headers['content-type'])
            self.assertIn(b'name="image"', req.content)
            self.assertIn(b'fixture', req.content)
            return httpx.Response(200, json={'data': [{'b64_json': encoded}]})
        response = self.call('image', {'provider': 'moyuu', 'apiKey': 'image-only', 'model': 'nanobanana', 'prompt': '封面', 'size': '1024x1024', 'referenceImage': 'data:image/png;base64,' + encoded}, upstream)
        self.assertEqual(response.status_code, 200)
        self.assertTrue(response.json()['image'].startswith('data:image/png;base64,'))
        self.assertTrue(response.json()['usedReference'])

    def test_invalid_reference_and_missing_key_rejected(self):
        for body in [{'provider': 'moyuu', 'apiKey': ''}, {'provider': 'moyuu', 'apiKey': 'key', 'model': 'x', 'prompt': 'x', 'referenceImage': 'data:image/svg+xml;base64,PHN2Zz4='}]:
            response = self.call('image', body, lambda _: self.fail('network called'))
            self.assertEqual(response.status_code, 400)

    def test_unknown_image_url_protocol_rejected(self):
        response = self.call('image', {'provider': 'moyuu', 'apiKey': 'key', 'model': 'x', 'prompt': 'x'}, lambda _: httpx.Response(200, json={'data': [{'url': 'javascript:alert(1)'}]}))
        self.assertEqual(response.status_code, 502)

    def test_only_provider_auth_failures_use_key_fallback_code(self):
        for status in (402, 403, 429, 500):
            response = self.call('text', {'provider': 'deepseek', 'apiKey': 'fixture', 'model': 'deepseek-v4-flash', 'prompt': 'JSON'}, lambda _, code=status: httpx.Response(code, text='error'))
            self.assertEqual(response.json()['code'], {402: 'AI_QUOTA_EXCEEDED', 403: 'AI_KEY_INVALID', 429: 'AI_RATE_LIMITED'}.get(status, 'AI_UPSTREAM_ERROR'))
