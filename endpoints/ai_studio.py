"""Explicit, stateless AI requests; credentials never enter application storage."""
import base64
import binascii
import json
from urllib.parse import urlsplit

import httpx
from fastapi import APIRouter, FastAPI, Request
from fastapi.responses import JSONResponse

router = APIRouter()
MAX_RESPONSE = 48 * 1024 * 1024
MAX_REQUEST = 12 * 1024 * 1024


class StudioError(Exception):
    def __init__(self, status, code, message):
        self.status, self.code, self.message = status, code, message


def fail(message, status=400, code='AI_INVALID_REQUEST'):
    raise StudioError(status, code, message)


def make_client():
    return httpx.AsyncClient(timeout=httpx.Timeout(240, connect=20), follow_redirects=False)


def endpoint(body):
    provider = body.get('provider')
    if provider == 'deepseek':
        return 'https://api.deepseek.com'
    if provider == 'moyuu':
        return 'https://moyuu.cc/v1'
    if provider != 'jucodex':
        fail('请选择支持的 AI 服务商。')
    raw = body.get('baseUrl', '')
    if not isinstance(raw, str):
        fail('请填写 Jucodex API 基地址。')
    try:
        parsed = urlsplit(raw.strip())
        valid = parsed.scheme == 'https' and (parsed.hostname == 'jucodex.com' or (parsed.hostname or '').endswith('.jucodex.com')) and not parsed.username and not parsed.password and not parsed.query and not parsed.fragment and parsed.port in (None, 443)
    except ValueError:
        valid = False
    if not valid:
        fail('Jucodex 基地址须为其 HTTPS 域名，不得包含账号、密钥或查询参数。')
    return raw.strip().rstrip('/')


def field(body, name, maximum=1000, required=True):
    value = body.get(name, '')
    if not isinstance(value, str) or len(value) > maximum or (required and not value.strip()):
        fail('请检查必填内容、模型和 API Key 是否完整，或缩短输入内容。')
    return value.strip()


def image_type(raw):
    if raw.startswith(b'\x89PNG\r\n\x1a\n'):
        return 'image/png'
    if raw.startswith(b'\xff\xd8\xff'):
        return 'image/jpeg'
    if raw.startswith(b'RIFF') and raw[8:12] == b'WEBP':
        return 'image/webp'
    return None


def reference_image(value):
    try:
        header, encoded = value.split(',', 1)
        if header not in ('data:image/png;base64', 'data:image/jpeg;base64', 'data:image/webp;base64'):
            fail('源封面仅支持 JPG、PNG、WebP。')
        raw = base64.b64decode(encoded, validate=True)
        mime = image_type(raw)
        if not raw or len(raw) > 8 * 1024 * 1024 or not mime:
            fail('源封面无效或超过 8 MB。')
        return raw, mime
    except (ValueError, binascii.Error, AttributeError):
        fail('源封面编码无效，请重新选择图片。')


async def upstream_json(client, method, url, key, **kwargs):
    try:
        async with client.stream(method, url, headers={'Authorization': 'Bearer ' + key}, **kwargs) as response:
            status = response.status_code
            if not 200 <= status < 300:
                messages = {
                    401: 'API Key 无效或已过期，请检查对应服务的 Key。',
                    402: '服务商账户额度不足，请检查余额。',
                    403: '服务商拒绝访问，请检查 Key 的模型权限或地区限制。',
                    404: '服务商未提供此模型或接口，请核对模型和基地址；参考图模式需要 images/edits。',
                    429: '服务商限流或额度不足，请稍后重试并检查额度。',
                    451: '服务商限制当前地区访问。',
                }
                fail(messages.get(status, f'服务商返回 HTTP {status}，请检查模型、尺寸及接口支持情况。'), status if status in (401, 402, 403, 429, 451) else 502, 'AI_KEY_INVALID' if status in (401, 403) else 'AI_UPSTREAM_ERROR')
            chunks, size = [], 0
            async for chunk in response.aiter_bytes():
                size += len(chunk)
                if size > MAX_RESPONSE:
                    fail('服务商响应过大，请降低图片尺寸。', 502, 'AI_RESPONSE_TOO_LARGE')
                chunks.append(chunk)
            try:
                value = json.loads(b''.join(chunks))
            except (ValueError, UnicodeDecodeError):
                fail('服务商未返回有效 JSON；请核对 API 基地址。', 502, 'AI_INVALID_RESPONSE')
            if not isinstance(value, dict) or 'error' in value:
                fail('服务商响应格式异常，请检查模型权限和接口配置。', 502, 'AI_INVALID_RESPONSE')
            return value
    except httpx.TimeoutException:
        fail('请求超时，未自动重试；服务商可能已扣费，请检查后再试。', 504, 'AI_TIMEOUT')
    except httpx.HTTPError:
        fail('无法连接 AI 服务，请检查网络或代理配置。', 502, 'AI_NETWORK_ERROR')


def parse_text(value):
    try:
        raw = value['choices'][0]['message']['content']
        if not isinstance(raw, str):
            raise ValueError()
        if raw.strip().startswith('```'):
            raw = '\n'.join(raw.strip().splitlines()[1:-1])
        result = json.loads(raw)
        candidates = result['title_candidates']
        concepts = result['cover_concepts']
        if not isinstance(candidates, list) or not 1 <= len(candidates) <= 10:
            raise ValueError()
        for candidate in candidates:
            if not isinstance(candidate, dict) or not isinstance(candidate.get('title'), str) or not 1 <= len(candidate['title']) <= 100:
                raise ValueError()
        if not isinstance(result['description'], str) or len(result['description']) > 5000:
            raise ValueError()
        if not isinstance(result.get('recommended_title'), str) or result['recommended_title'] not in [c['title'] for c in candidates]:
            result['recommended_title'] = candidates[0]['title']
        if not isinstance(concepts, list) or not 1 <= len(concepts) <= 5:
            raise ValueError()
        for concept in concepts:
            if not isinstance(concept, dict) or not isinstance(concept.get('prompt'), str) or not concept['prompt'].strip():
                raise ValueError()
        tags = result.get('tags', [])
        if not isinstance(tags, list) or not all(isinstance(tag, str) for tag in tags):
            raise ValueError()
        return result
    except (KeyError, TypeError, ValueError, IndexError):
        fail('文字模型返回内容不符合标题、描述或封面方案格式。请重试；未使用演示结果替代。', 502, 'AI_INVALID_CONTENT')


async def execute(action, body):
    if action not in ('models', 'text', 'image') or not isinstance(body, dict):
        fail('不支持的 AI 操作。')
    base = endpoint(body)
    if isinstance(body.get('apiKey', ''), str) and not body.get('apiKey', '').strip():
        fail('未填写 AI 服务 API Key。', 400, 'AI_KEY_MISSING')
    key = field(body, 'apiKey', 1024)
    if any(c.isspace() for c in key) or not key.isascii():
        fail('API Key 格式不正确，请检查是否多复制了空格或换行。', 400, 'AI_KEY_INVALID')
    async with make_client() as client:
        if action == 'models':
            value = await upstream_json(client, 'GET', base + '/models', key)
            entries = value.get('data')
            if not isinstance(entries, list):
                fail('模型列表格式异常。', 502, 'AI_INVALID_RESPONSE')
            models = sorted({v['id'] for v in entries if isinstance(v, dict) and isinstance(v.get('id'), str) and len(v['id']) <= 200})
            return {'models': models}
        model = field(body, 'model', 200)
        if body.get('provider') == 'deepseek' and model == 'deepseek-flash':
            model = 'deepseek-v4-flash'
        prompt = field(body, 'prompt', 100000)
        if action == 'text':
            if body.get('provider') not in ('deepseek', 'jucodex'):
                fail('文字请求只能使用文字服务 Key。')
            if body.get('provider') == 'deepseek' and model not in ('deepseek-v4-pro', 'deepseek-v4-flash'):
                fail('请选择 DeepSeek V4 Pro 或 Flash。')
            payload = {'model': model, 'stream': False, 'messages': [{'role': 'system', 'content': '按用户要求返回一个 JSON 对象，字段类型必须符合约定；不要输出 Markdown。'}, {'role': 'user', 'content': prompt}], 'response_format': {'type': 'json_object'}, 'max_tokens': 8192}
            if body.get('provider') == 'deepseek':
                payload['thinking'] = {'type': 'disabled'}
            value = await upstream_json(client, 'POST', base + '/chat/completions', key, json=payload)
            return {'result': parse_text(value), 'model': model}
        if body.get('provider') not in ('moyuu', 'jucodex'):
            fail('封面请求只能使用封面服务 Key。')
        size = body.get('size', '1024x1024')
        if size not in ('auto', '1024x1024', '1536x864', '1536x1024', '1792x1024', '1024x576'):
            fail('不支持的封面尺寸。')
        reference = body.get('referenceImage')
        payload = {'model': model, 'prompt': prompt, 'size': size, 'n': 1}
        if reference:
            raw, mime = reference_image(reference)
            ext = {'image/png': 'png', 'image/jpeg': 'jpg', 'image/webp': 'webp'}[mime]
            value = await upstream_json(client, 'POST', base + '/images/edits', key, data={k: str(v) for k, v in payload.items()}, files={'image': ('reference.' + ext, raw, mime)})
        else:
            value = await upstream_json(client, 'POST', base + '/images/generations', key, json=payload)
        try:
            item = value['data'][0]
            if item.get('b64_json'):
                raw = base64.b64decode(item['b64_json'], validate=True)
                mime = image_type(raw)
                if not mime:
                    raise ValueError()
                image = 'data:' + mime + ';base64,' + item['b64_json']
            else:
                image = item['url']
                parsed = urlsplit(image)
                if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password:
                    raise ValueError()
            return {'image': image, 'model': model, 'usedReference': bool(reference)}
        except (KeyError, ValueError, TypeError, IndexError, binascii.Error):
            fail('图片接口未返回有效图片或 HTTPS 图片地址。', 502, 'AI_INVALID_IMAGE')


@router.post('/studio/{action}')
async def studio(action: str, request: Request):
    try:
        # The desktop bridge has no Origin; browser requests must come through the local UI.
        origin = request.headers.get('origin')
        if origin:
            parsed = urlsplit(origin)
            if not (parsed.scheme == 'http' and parsed.hostname in ('localhost', '127.0.0.1') and parsed.port in (1420, 1424)):
                fail('不允许此网页发起 AI 请求。', 403, 'AI_ORIGIN_DENIED')
        chunks, size = [], 0
        async for chunk in request.stream():
            size += len(chunk)
            if size > MAX_REQUEST:
                fail('请求过大，请缩小源封面。', 413)
            chunks.append(chunk)
        try:
            body = json.loads(b''.join(chunks))
        except (ValueError, UnicodeDecodeError):
            fail('请求格式无效。')
        return JSONResponse(await execute(action, body), headers={'Cache-Control': 'no-store'})
    except StudioError as error:
        return JSONResponse({'code': error.code, 'message': error.message}, status_code=error.status, headers={'Cache-Control': 'no-store'})
    except Exception:
        # Never return request bodies, provider responses, exception text or credentials.
        return JSONResponse({'code': 'AI_INTERNAL_ERROR', 'message': 'AI 请求处理失败，请检查配置后重试。'}, status_code=500, headers={'Cache-Control': 'no-store'})


# A lightweight loopback server for Vite, sharing the production request implementation.
preview_app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
preview_app.include_router(router, prefix='/api')
