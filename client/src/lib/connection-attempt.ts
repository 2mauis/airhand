// Connection replies and stream events are independent deliveries. A terminal
// event invalidates its whole attempt, including a success reply still in flight.
export class ConnectionAttempt {
  private generation = 0;
  private pending: number | null = null;

  begin(): number {
    this.pending = ++this.generation;
    return this.generation;
  }
  accepts(generation: number): boolean { return generation === this.generation; }
  finish(generation: number) {
    if (this.pending === generation) this.pending = null;
  }
  invalidate() {
    this.generation++;
    this.pending = null;
  }
  get attempting(): boolean { return this.pending !== null; }
}
