/** A model-facing failure with an explicit retry decision. */
export class ToolFailure extends Error {
  constructor(type, message, retryable = false, httpStatus) {
    super(message);
    this.detail = { type, message, retryable, ...(httpStatus === undefined ? {} : { http_status: httpStatus }) };
  }

  toResult(field = 'code') {
    const { type, ...detail } = this.detail;
    return { status: 'error', error: { [field]: type, ...detail } };
  }
}
