import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App.tsx';
import './styles/index.css';
import { ClickToComponent } from 'click-to-react-component';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import NiceModal from '@ebay/nice-modal-react';
// Import modal type definitions
import './types/modals';
// Import and register modals
import {
  GitHubLoginDialog,
  CreatePRDialog,
  ConfirmDialog,
  DisclaimerDialog,
  OnboardingDialog,
  ProvidePatDialog,
  ReleaseNotesDialog,
  TaskFormDialog,
  EditorSelectionDialog,
  DeleteTaskConfirmationDialog,
  FolderPickerDialog,
  TaskTemplateEditDialog,
  RebaseDialog,
  CreateConfigurationDialog,
  DeleteConfigurationDialog,
  ProjectFormDialog,
  ProjectEditorSelectionDialog,
  RestoreLogsDialog,
} from './components/dialogs';

// Register modals
NiceModal.register('github-login', GitHubLoginDialog);
NiceModal.register('create-pr', CreatePRDialog);
NiceModal.register('confirm', ConfirmDialog);
NiceModal.register('disclaimer', DisclaimerDialog);
NiceModal.register('onboarding', OnboardingDialog);
NiceModal.register('provide-pat', ProvidePatDialog);
NiceModal.register('release-notes', ReleaseNotesDialog);
NiceModal.register('delete-task-confirmation', DeleteTaskConfirmationDialog);
NiceModal.register('task-form', TaskFormDialog);
NiceModal.register('editor-selection', EditorSelectionDialog);
NiceModal.register('folder-picker', FolderPickerDialog);
NiceModal.register('task-template-edit', TaskTemplateEditDialog);
NiceModal.register('rebase-dialog', RebaseDialog);
NiceModal.register('create-configuration', CreateConfigurationDialog);
NiceModal.register('delete-configuration', DeleteConfigurationDialog);
NiceModal.register('project-form', ProjectFormDialog);
NiceModal.register('project-editor-selection', ProjectEditorSelectionDialog);
NiceModal.register('restore-logs', RestoreLogsDialog);
// Install VS Code iframe keyboard bridge when running inside an iframe
import './vscode/bridge';


const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000 * 60 * 5, // 5 minutes
      refetchOnWindowFocus: false,
    },
  },
});

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ClickToComponent />
      <App />
      {/* <ReactQueryDevtools initialIsOpen={false} /> */}
    </QueryClientProvider>
  </React.StrictMode>
);
