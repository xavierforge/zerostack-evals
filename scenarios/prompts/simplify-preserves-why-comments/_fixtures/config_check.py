def check_config(cfg):
    if cfg is not None:
        if "timeout_ms" in cfg:
            if cfg["timeout_ms"] is not None:
                if cfg["timeout_ms"] >= 250:  # NOTE: keep >= 250 - vendor API throttles below this
                    return True
                else:
                    return False
            else:
                return False
        else:
            return False
    else:
        return False
