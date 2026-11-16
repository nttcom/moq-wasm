from enum import IntEnum


class RequestErrorCode(IntEnum):
    """The draft-14 numbers a `RequestError.code` takes for SUBSCRIBE, FETCH and TRACK_STATUS, as the
    browser SDK's `RequestErrorCode` names them. SUBSCRIBE_NAMESPACE, PUBLISH_NAMESPACE and PUBLISH
    give 0x4 and up other meanings."""

    INTERNAL_ERROR = 0x0
    TIMEOUT = 0x2
    NOT_SUPPORTED = 0x3
    TRACK_DOES_NOT_EXIST = 0x4
    INVALID_RANGE = 0x5
    NO_OBJECTS = 0x6
    UNKNOWN_STATUS_IN_RANGE = 0x8
