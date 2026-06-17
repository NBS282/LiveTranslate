from lt_engine.pipeline import normalize_lang
import pytest


def test_short_code_maps_to_nllb():
    assert normalize_lang("es") == "spa_Latn"
    assert normalize_lang("en") == "eng_Latn"


def test_full_code_passthrough():
    assert normalize_lang("por_Latn") == "por_Latn"


def test_unknown_short_code_raises():
    with pytest.raises(ValueError):
        normalize_lang("zz")
