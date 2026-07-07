from config import Config


def test_config_defaults():
    c = Config(host="test", port=1234)
    assert c.debug is False
