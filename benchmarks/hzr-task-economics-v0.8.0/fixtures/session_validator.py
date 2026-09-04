"""Tiny offline fixture; not a representative production task suite."""
def validate(session, now):
    if session["revoked"]:
        return 403
    if session["expires_at"] <= now:
        return 401
    return 200


def refresh(session, now):
    if session["revoked"]:
        return 403
    if session["expires_at"] < now:
        return 401
    return 200
