// @vitest-environment happy-dom

import React from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import ExportCommitmentsModal from '@/components/export/ExportCommitmentsModal';

function renderModal(props: Partial<React.ComponentProps<typeof ExportCommitmentsModal>> = {}) {
  return render(
    <ExportCommitmentsModal
      isOpen={true}
      onClose={vi.fn()}
      ownerAddress="GOWNERADDRESS"
      sessionToken="session-token"
      {...props}
    />
  );
}

describe('ExportCommitmentsModal', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    window.sessionStorage.clear();
    window.localStorage.clear();

    vi.stubGlobal('fetch', vi.fn());
    Object.defineProperty(window.URL, 'createObjectURL', {
      configurable: true,
      value: vi.fn(() => 'blob:commitments'),
    });
    Object.defineProperty(window.URL, 'revokeObjectURL', {
      configurable: true,
      value: vi.fn(),
    });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  });

  it('calls the export endpoint with the owner address and session token, then downloads the CSV', async () => {
    vi.mocked(fetch).mockResolvedValue(
      new Response('Commitment ID,Owner\r\ncommitment-1,GOWNERADDRESS\r\n', {
        status: 200,
        headers: {
          'content-disposition': 'attachment; filename="commitments.csv"',
          'content-type': 'text/csv; charset=utf-8',
        },
      })
    );

    renderModal();

    fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }));

    await waitFor(() => {
      expect(fetch).toHaveBeenCalledWith(
        'http://localhost:3000/api/commitments/export?ownerAddress=GOWNERADDRESS',
        {
          method: 'GET',
          headers: {
            Authorization: 'Bearer session-token',
          },
        }
      );
    });

    expect(HTMLAnchorElement.prototype.click).toHaveBeenCalled();
    expect(screen.getByText('Export ready. 1 commitment downloaded as CSV.')).toBeTruthy();
  });

  it('uses a stored session token when one is available', async () => {
    window.sessionStorage.setItem('commitlabs.sessionToken', 'stored-token');
    vi.mocked(fetch).mockResolvedValue(
      new Response('Commitment ID,Owner\r\n', {
        status: 200,
        headers: {
          'content-disposition': 'attachment; filename="commitments.csv"',
          'content-type': 'text/csv; charset=utf-8',
        },
      })
    );

    renderModal({ sessionToken: undefined });

    fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }));

    await waitFor(() => {
      expect(fetch).toHaveBeenCalledWith(
        'http://localhost:3000/api/commitments/export?ownerAddress=GOWNERADDRESS',
        expect.objectContaining({
          headers: {
            Authorization: 'Bearer stored-token',
          },
        })
      );
    });
  });

  it('reports an empty CSV export without treating it as a failure', async () => {
    vi.mocked(fetch).mockResolvedValue(
      new Response('Commitment ID,Owner\r\n', {
        status: 200,
        headers: {
          'content-disposition': 'attachment; filename="commitments.csv"',
          'content-type': 'text/csv; charset=utf-8',
        },
      })
    );

    renderModal();

    fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }));

    await screen.findByText(
      'Export ready. No commitment rows found, so a header-only CSV was downloaded.'
    );
  });

  it('shows a sign-in error before calling the endpoint when no session token exists', async () => {
    renderModal({ sessionToken: undefined });

    fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Sign in again before exporting your commitments.');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('closes with Escape when it is not preparing an export', () => {
    const onClose = vi.fn();
    renderModal({ onClose });

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('keeps keyboard focus inside the dialog', () => {
    renderModal();

    const dialog = screen.getByRole('dialog');
    const closeButton = screen.getByRole('button', { name: 'Close export dialog' });
    const exportButton = screen.getByRole('button', { name: 'Export CSV' });

    exportButton.focus();
    fireEvent.keyDown(dialog, { key: 'Tab' });

    expect(document.activeElement).toBe(closeButton);
  });
});
