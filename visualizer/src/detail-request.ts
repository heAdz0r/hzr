export interface DetailRequestTicket {
  readonly generation: number;
  readonly projectId: string | null;
  readonly signal: AbortSignal;
}

/**
 * Owns one in-flight project request and makes project changes a hard boundary.
 * A response may update the UI only while its ticket is current.
 */
export class LatestProjectRequestCoordinator {
  private generation = 0;
  private projectId: string | null = null;
  private controller: AbortController | null = null;

  switchProject(projectId: string | null): void {
    this.controller?.abort();
    this.controller = null;
    this.projectId = projectId;
    this.generation += 1;
  }

  begin(projectId: string | null): DetailRequestTicket {
    if (projectId !== this.projectId) this.switchProject(projectId);
    this.controller?.abort();
    this.controller = new AbortController();
    this.generation += 1;
    return {
      generation: this.generation,
      projectId,
      signal: this.controller.signal,
    };
  }

  isCurrent(ticket: DetailRequestTicket, projectId: string | null): boolean {
    return (
      ticket.generation === this.generation &&
      ticket.projectId === this.projectId &&
      projectId === this.projectId &&
      !ticket.signal.aborted
    );
  }

  finish(ticket: DetailRequestTicket): void {
    if (ticket.generation === this.generation) this.controller = null;
  }

  abort(): void {
    this.switchProject(this.projectId);
  }
}

export { LatestProjectRequestCoordinator as DetailRequestCoordinator };

export function isCurrentProjectSnapshot<T>(
  requests: LatestProjectRequestCoordinator,
  ticket: DetailRequestTicket,
  projectId: string | null,
  capturedSnapshot: T,
  currentSnapshot: T,
): boolean {
  return requests.isCurrent(ticket, projectId) && capturedSnapshot === currentSnapshot;
}
