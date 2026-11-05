// Audio/state changes happen at the call site. This queue is only for painting:
// one latest-state refresh per frame, with no animation timer in the audio path.
export class ViewFeedback {
  constructor(
    paint,
    {
      request = (callback) => requestAnimationFrame(callback),
      cancel = (id) => cancelAnimationFrame(id),
    } = {},
  ) {
    this.paint = paint;
    this.request = request;
    this.cancel = cancel;
    this.pending = null;
  }
  schedule() {
    if (this.pending !== null) return;
    this.pending = this.request(() => {
      this.pending = null;
      this.paint();
    });
  }
  dispose() {
    if (this.pending !== null) this.cancel(this.pending);
    this.pending = null;
  }
}
