import React, { useState } from 'react';
import SettlementModal, { SettlementState } from './SettlementModal';

const SettlementPage: React.FC = () => {
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [modalState, setModalState] = useState<SettlementState>('eligible');
  const [step, setStep] = useState<0 | 1 | 2>(0);

  const commitmentDetails = {
    id: 'TX-88294-STELLAR',
    amount: '5,000.00',
    asset: 'XLM',
  };

  const handleStartSettlement = async () => {
    setModalState('processing');
    setStep(0);

    // Simulate the settlement lifecycle (Initiating -> Confirming -> Finalizing)
    setTimeout(() => setStep(1), 2000); 
    setTimeout(() => setStep(2), 4000); 
    setTimeout(() => setModalState('settled'), 6000); 
  };

  const handleClose = () => {
    setIsModalOpen(false);
    // Reset the internal flow state after the modal closes
    setTimeout(() => {
      setModalState('eligible');
      setStep(0);
    }, 300);
  };

  return (
    <div className="min-h-screen bg-[#050505] flex flex-col items-center justify-center p-6">
      <div className="max-w-xl w-full text-center space-y-8">
        <h1 className="text-4xl font-bold text-white tracking-tight">Commitment Management</h1>
        <p className="text-white/60">Manage your active commitments and trigger settlements upon maturity.</p>
        
        <button
          onClick={() => setIsModalOpen(true)}
          className="px-8 py-3 bg-white text-black font-bold rounded-lg hover:bg-gray-200 transition-all active:scale-95"
        >
          Open Settlement Workflow
        </button>
      </div>

      <SettlementModal
        isOpen={isModalOpen}
        onClose={handleClose}
        state={modalState}
        processingStep={step}
        commitmentDetails={commitmentDetails}
        onSettlementStart={handleStartSettlement}
        onReturnToDashboard={handleClose}
      />
    </div>
  );
};

export default SettlementPage;