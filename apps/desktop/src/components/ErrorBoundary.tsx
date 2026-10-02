import { Component, type ReactNode } from "react";
import { i18next } from "../i18n";

/** Keeps a failing lazy panel (e.g. chunk load error) from taking the shell down. */
export class ErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (this.state.failed) {
      return (
        <div className="panel" role="alert">
          <p>{i18next.t("error.panel")}</p>
          <button type="button" className="btn btn-outline" onClick={() => this.setState({ failed: false })}>
            {i18next.t("common.retry")}
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
