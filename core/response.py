try:
    import orjson
except ImportError:
    orjson = None

from fastapi.responses import JSONResponse, ORJSONResponse

ResponseClass = ORJSONResponse if orjson is not None else JSONResponse


def success(data, msg: str = "ok"):
    return ResponseClass({"code": 0, "msg": msg, "data": data})


def error(msg: str, code: int = -1, status_code: int = 400):
    return ResponseClass(
        {"code": code, "msg": msg, "data": None},
        status_code=status_code,
    )
