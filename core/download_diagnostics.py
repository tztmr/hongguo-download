"""Small local diagnostics without URLs, keys, payloads or exception messages."""
import json
import logging
import os
import secrets
import traceback
from datetime import datetime, timezone
from logging.handlers import RotatingFileHandler
from pathlib import Path


def record_failure(item_id, stage, exc):
    reference = secrets.token_hex(4)
    entry = dict(at=datetime.now(timezone.utc).isoformat(), reference=reference,
                 item_id=item_id, stage=stage, error_type=type(exc).__name__,
                 frames=[dict(file=Path(f.filename).name, line=f.lineno, function=f.name)
                         for f in traceback.extract_tb(exc.__traceback__)][-12:])
    try:
        root = Path(os.environ.get('HONGGUO_DATA_DIR') or Path(__file__).resolve().parent.parent / 'config')
        folder = root / 'logs'
        folder.mkdir(parents=True, exist_ok=True)
        handler = RotatingFileHandler(folder / 'download-errors.jsonl', maxBytes=1024 * 1024, backupCount=2, encoding='utf-8')
        try:
            handler.emit(logging.LogRecord('download', logging.ERROR, '', 0, json.dumps(entry, ensure_ascii=False), (), None))
        finally:
            handler.close()
    except OSError:
        pass
    return reference
