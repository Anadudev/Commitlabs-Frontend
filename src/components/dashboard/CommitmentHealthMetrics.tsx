'use client';

import React, { useRef, useState } from 'react';
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';
import dynamic from 'next/dynamic';
import { TrendingUp, TrendingDown, DollarSign, CheckCircle } from 'lucide-react';
import HealthMetricsSkeleton from '../HealthMetricsSkeleton';
import { ChartExportMenu } from './ChartExportMenu';
import type { HealthMetricsExportData } from '@/utils/chartExport';

function cn(...inputs: ClassValue[]) {
    return twMerge(clsx(inputs));
}

const HealthMetricsValueHistoryChart = dynamic(
    () => import('./HealthMetricsValueHistoryChart').then((mod) => mod.HealthMetricsValueHistoryChart),
    { ssr: false, loading: () => <HealthMetricsSkeleton /> }
);

const HealthMetricsDrawdownChart = dynamic(
    () => import('./HealthMetricsDrawdownChart').then((mod) => mod.HealthMetricsDrawdownChart),
    { ssr: false, loading: () => <HealthMetricsSkeleton /> }
);

const HealthMetricsFeeGenerationChart = dynamic(
    () => import('./HealthMetricsFeeGenerationChart').then((mod) => mod.HealthMetricsFeeGenerationChart),
    { ssr: false, loading: () => <HealthMetricsSkeleton /> }
);

const HealthMetricsComplianceChart = dynamic(
    () => import('./HealthMetricsComplianceChart').then((mod) => mod.HealthMetricsComplianceChart),
    { ssr: false, loading: () => <HealthMetricsSkeleton /> }
);

type TabType = 'value' | 'drawdown' | 'fee' | 'compliance';

const tabIcons: Record<TabType, React.ReactNode> = {
    value: <TrendingUp className="w-4 h-4" />,
    drawdown: <TrendingDown className="w-4 h-4" />,
    fee: <DollarSign className="w-4 h-4" />,
    compliance: <CheckCircle className="w-4 h-4" />,
};

interface CommitmentHealthMetricsProps {
    commitmentId: string;
    complianceData: Array<{ date: string; complianceScore: number }>;
    drawdownData: Array<{ date: string; drawdownPercent: number }>;
    valueHistoryData: Array<{ date: string; currentValue: number; initialAmount?: number }>;
    feeGenerationData: Array<{ date: string; feeAmount: number }>;
    thresholdPercent?: number;
    volatilityPercent?: number;
    isLoading?: boolean;
}

export default function CommitmentHealthMetrics({
    commitmentId,
    complianceData,
    drawdownData,
    valueHistoryData,
    feeGenerationData,
    thresholdPercent,
    volatilityPercent,
    isLoading = false,
}: CommitmentHealthMetricsProps) {
    const [activeTab, setActiveTab] = useState<TabType>('value');
    const valueChartRef = useRef<HTMLDivElement>(null);
    const drawdownChartRef = useRef<HTMLDivElement>(null);
    const feeChartRef = useRef<HTMLDivElement>(null);
    const complianceChartRef = useRef<HTMLDivElement>(null);

    const exportData: HealthMetricsExportData = {
        complianceData,
        drawdownData,
        valueHistoryData,
        feeGenerationData,
    };

    const tabs: { id: TabType; label: string }[] = [
        { id: 'value', label: 'Value History' },
        { id: 'drawdown', label: 'Drawdown' },
        { id: 'fee', label: 'Fee Generation' },
        { id: 'compliance', label: 'Compliance' },
    ];

    return (
        <div className="w-full bg-[#0a0a0a] rounded-2xl p-6 border border-[#222]">
            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-8">
                <h2 className="text-2xl font-semibold text-white">Health Metrics</h2>

                <div className="flex flex-wrap gap-2 p-1 bg-[#111] rounded-lg border border-[#222]">
                    {tabs.map((tab) => (
                        <button
                            key={tab.id}
                            onClick={() => setActiveTab(tab.id)}
                            className={cn(
                                'flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-md transition-all duration-200',
                                activeTab === tab.id
                                    ? 'bg-[#222] text-[#0ff0fc] shadow-sm'
                                    : 'text-[#8892a0] hover:text-[#99a1af] hover:bg-[#1a1a1a]'
                            )}
                        >
                            {tabIcons[tab.id]}
                            {tab.label}
                        </button>
                    ))}
                </div>
            </div>

            <div className="w-full">
                {activeTab === 'value' && (
                    <div className="relative" ref={valueChartRef}>
                        <ChartExportMenu
                            commitmentId={commitmentId}
                            tab="value"
                            data={exportData}
                            disabled={isLoading}
                            chartContainerRef={valueChartRef}
                        />
                        <HealthMetricsValueHistoryChart
                            data={valueHistoryData}
                            volatilityPercent={volatilityPercent}
                        />
                    </div>
                )}
                {activeTab === 'drawdown' && (
                    <div className="relative" ref={drawdownChartRef}>
                        <ChartExportMenu
                            commitmentId={commitmentId}
                            tab="drawdown"
                            data={exportData}
                            disabled={isLoading}
                            chartContainerRef={drawdownChartRef}
                        />
                        <HealthMetricsDrawdownChart
                            data={drawdownData}
                            thresholdPercent={thresholdPercent}
                            volatilityPercent={volatilityPercent}
                        />
                    </div>
                )}
                {activeTab === 'fee' && (
                    <div className="relative" ref={feeChartRef}>
                        <ChartExportMenu
                            commitmentId={commitmentId}
                            tab="fee"
                            data={exportData}
                            disabled={isLoading}
                            chartContainerRef={feeChartRef}
                        />
                        <HealthMetricsFeeGenerationChart
                            data={feeGenerationData}
                            volatilityPercent={volatilityPercent}
                        />
                    </div>
                )}
                {activeTab === 'compliance' && (
                    <div className="relative" ref={complianceChartRef}>
                        <ChartExportMenu
                            commitmentId={commitmentId}
                            tab="compliance"
                            data={exportData}
                            disabled={isLoading}
                            chartContainerRef={complianceChartRef}
                        />
                        <HealthMetricsComplianceChart data={complianceData} />
                    </div>
                )}
            </div>
        </div>
    );
}
