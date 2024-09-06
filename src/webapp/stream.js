/**
 * Common class for pipeline elements
 */
class StreamPipelineElem {
  constructor() {
    this.next = null;
  }

  then = (handler) => {
    const next = new StreamThen(handler);
    this.next = next;
    return next;
  };
  map = (transform) => {
    const next = new StreamMap(transform);
    this.next = next;
    return next;
  };
  catch = (handler) => {
    const next = new StreamCatch(handler);
    this.next = next;
    return next;
  };
  finally = (handler) => {
    const next = new StreamFinally(handler);
    this.next = next;
    return next;
  };
}

/**
 * Apply a then to the stream pipeline
 */
class StreamThen extends StreamPipelineElem {
  constructor(handler) {
    super();
    this.handler = handler;
  }

  onMessage = (item) => {
    try {
      this.handler(item);
    } catch (error) {
      this.onError(error);
      return; // stop here
    }
    // propagate further
    if (this.next !== null) {
      this.next.onMessage(item);
    }
  };

  onError = (error) => {
    if (this.next !== null) {
      this.next.onError(error);
    } else {
      throw error;
    }
  };

  onFinally = () => {
    if (this.next !== null) {
      this.next.onFinally();
    }
  };
}

/**
 * Apply a map to the stream pipeline
 */
class StreamMap extends StreamPipelineElem {
  constructor(transform) {
    super();
    this.transform = transform;
  }

  onMessage = (item) => {
    let transformed = null;
    try {
      transformed = this.transform(item);
    } catch (error) {
      this.onError(error);
      return; // stop here
    }
    // propagate further
    if (this.next !== null) {
      this.next.onMessage(transformed);
    }
  };

  onError = (error) => {
    if (this.next !== null) {
      this.next.onError(error);
    } else {
      throw error;
    }
  };

  onFinally = () => {
    if (this.next !== null) {
      this.next.onFinally();
    }
  };
}

/**
 * Apply a catch to the stream pipeline
 */
class StreamCatch extends StreamPipelineElem {
  constructor(handler) {
    super();
    this.handler = handler;
  }

  onMessage = (item) => {
    if (this.next !== null) {
      this.next.onMessage(item);
    }
  };

  onError = (error) => {
    try {
      this.handler(error);
    } catch (otherError) {
      if (this.next !== null) {
        this.next.onError(otherError);
      } else {
        throw otherError;
      }
    }
  };

  onFinally = () => {
    if (this.next !== null) {
      this.next.onFinally();
    }
  };
}

/**
 * Apply a finally to the stream pipeline
 */
class StreamFinally extends StreamPipelineElem {
  constructor(handler) {
    super();
    this.handler = handler;
  }

  onMessage = (item) => {
    if (this.next !== null) {
      this.next.onMessage(item);
    }
  };

  onError = (error) => {
    if (this.next !== null) {
      this.next.onError(error);
    } else {
      throw error;
    }
  };

  onFinally = () => {
    try {
      this.handler();
    } catch (error) {
      this.onError(error);
      return; // stop here
    }
    // continue propagation
    if (this.next !== null) {
      this.next.onFinally();
    }
  };
}

/**
 * The source of a stream of events
 */
class StreamEventSource extends StreamPipelineElem {
  /**
   * Creates a new stream source
   * @param uri The base URI
   * @param parameters The base arguments
   * @param noReconnect Whether to close the source on error and not reconnect
   */
  constructor(uri, parameters, noReconnect) {
    super();
    const finalUri = uriEnqueueArgs(uri, parameters);
    this.noReconnect = noReconnect ?? false;
    this.source = new EventSource(finalUri, { withCredentials: true });
    this.source.addEventListener("message", this.onMessage);
    this.source.addEventListener("error", this.onError);
  }

  onMessage = (message) => {
    if (this.next !== null) {
      this.next.onMessage(message);
    }
  };

  onError = (error) => {
    if (this.noReconnect) {
      this.source.close();
      this.onFinally();
    } else if (this.next !== null) {
      if (this.next !== null) {
        this.next.onError(error);
      }
    }
  };

  onFinally = () => {
    if (this.next !== null) {
      this.next.onFinally();
    }
  };
}

/**
 * Completes the uri with parameters
 * @param uri The URI for the API
 * @param parameters The parameter object, if any
 */
function uriEnqueueArgs(uri, parameters) {
  if (parameters !== null) {
    let names = Object.getOwnPropertyNames(parameters);
    let first = true;
    for (let p = 0; p !== names.length; p++) {
      let value = parameters[names[p]];
      if (Array.isArray(value)) {
        for (let i = 0; i !== value.length; i++) {
          uri += first ? "?" : "&";
          uri += names[p];
          uri += "=";
          uri += encodeURIComponent(value[i]);
          first = false;
        }
      } else {
        uri += first ? "?" : "&";
        uri += names[p];
        uri += "=";
        uri += encodeURIComponent(value);
      }
      first = false;
    }
  }
  return uri;
}
